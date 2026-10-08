//! Running one turn on a worker thread while the terminal keeps drawing.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;

use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};

use crate::agent::TraceEvent;
use crate::tui::render::{busy_editor_lines, place_editor_cursor};
use crate::tui::{
    stdout_supports_color, terminal_dimensions, CommandOutcome, DeliveryAction, EditorAction,
    EditorState, FollowUpQueue, TurnCtx,
};

use super::input::{InputOrMessage, TerminalInput};
use super::questions::prompt_user_question;
use super::{message_block, message_row, ReplRenderer};

mod events;
mod live;

use events::{prompt_tool_approval, trace_message, PendingQuestion, TurnMsg};
use live::{build_live, commit_reasoning};

/// Run a background turn while terminal and worker events wake the renderer.
/// Esc / Ctrl-C announce cancellation to the running operation.
// The renderer, editor, and follow-up queue are separate `&mut` borrows owned
// by the caller's loop, so no struct can group them without moving that state.
#[allow(clippy::too_many_arguments)]
pub(super) fn run_background_turn<H>(
    renderer: &mut ReplRenderer,
    handler: &H,
    prompt: &str,
    from_view: bool,
    attachments: &[super::super::Attachment],
    editor: &mut EditorState,
    queue: &mut FollowUpQueue,
    steering_available: bool,
    input: &mut TerminalInput,
) -> io::Result<(Result<CommandOutcome, String>, Vec<String>)>
where
    H: Fn(&str, &TurnCtx) -> Result<CommandOutcome, String> + Sync,
{
    let cancel = Arc::new(AtomicBool::new(false));
    // Note = the status line beside the skeleton; Delta = a live assistant text chunk.
    let (tx, mut rx) = futures::channel::mpsc::unbounded::<TurnMsg>();
    let mut note = String::from("working…");
    let mut streamed = String::new();
    let mut reasoning = String::new();
    let mut frame = 0usize;
    let mut tools_used: Vec<String> = Vec::new();
    let record_tool = |message: &str, tools: &mut Vec<String>| {
        if let Some(tool) = message.strip_prefix("tool: ") {
            let tool = tool.trim().to_string();
            if !tool.is_empty() && !tools.contains(&tool) {
                tools.push(tool);
            }
        }
    };
    // Committed blocks join the scrollback the main loop writes, at the
    // terminal's own width.
    let (columns, _) = terminal_dimensions()?;
    let scrollback_columns = columns;
    let color = stdout_supports_color();

    let outcome = thread::scope(|scope| -> io::Result<Result<CommandOutcome, String>> {
        let worker_cancel = cancel.clone();
        let note_tx = tx.clone();
        let delta_tx = tx.clone();
        let trace_tx = tx.clone();
        let approve_tx = tx.clone();
        let ask_tx = tx.clone();
        let worker = scope.spawn(move || {
            let progress = move |message: &str| {
                let _ = note_tx.unbounded_send(TurnMsg::Note(message.to_string()));
            };
            let stream = move |piece: &str| {
                let _ = delta_tx.unbounded_send(TurnMsg::Delta(piece.to_string()));
            };
            let trace = move |event: &TraceEvent<'_>| {
                let message = match *event {
                    TraceEvent::Reasoning { text } => TurnMsg::Reasoning(text.to_string()),
                    _ => match trace_message(event) {
                        Some(message) => TurnMsg::Trace(message),
                        None => return,
                    },
                };
                let _ = trace_tx.unbounded_send(message);
            };
            let approve = move |tool: &str, detail: &str| -> bool {
                let (reply, answer) = mpsc::channel::<bool>();
                if approve_tx
                    .unbounded_send(TurnMsg::Approve {
                        tool: tool.to_string(),
                        detail: detail.to_string(),
                        reply,
                    })
                    .is_err()
                {
                    return false;
                }
                answer.recv().unwrap_or(false)
            };
            let ask_user = move |question: &str, options: &[String]| -> Result<String, String> {
                let (reply, answer) = mpsc::channel::<Result<String, String>>();
                ask_tx
                    .unbounded_send(TurnMsg::AskUser {
                        question: question.to_string(),
                        options: options.to_vec(),
                        reply,
                    })
                    .map_err(|_| "Question channel closed".to_string())?;
                answer
                    .recv()
                    .unwrap_or_else(|_| Err("Question channel closed".into()))
            };
            let ctx = TurnCtx {
                cancel: worker_cancel,
                interactive: false,
                from_view,
                attachments,
                progress: &progress,
                stream: &stream,
                trace: &trace,
                ask_user: Some(&ask_user),
                approve: &approve,
            };
            handler(prompt, &ctx)
        });
        drop(tx);

        let render_result = (|| -> io::Result<()> {
            let mut pending = None;
            loop {
                let mut pending_approval: Option<(String, String, mpsc::Sender<bool>)> = None;
                let mut pending_question: Option<PendingQuestion> = None;
                let mut blocks = Vec::new();
                while let Some(message) = pending.take().or_else(|| rx.try_recv().ok()) {
                    match message {
                        TurnMsg::Note(m) => {
                            record_tool(&m, &mut tools_used);
                            note = m;
                        }
                        TurnMsg::Delta(p) => {
                            commit_reasoning(
                                &mut reasoning,
                                &mut blocks,
                                scrollback_columns,
                                color,
                            );
                            streamed.push_str(&p);
                        }
                        TurnMsg::Reasoning(p) => {
                            reasoning.push_str(&p);
                        }
                        TurnMsg::Trace(message) => {
                            commit_reasoning(
                                &mut reasoning,
                                &mut blocks,
                                scrollback_columns,
                                color,
                            );
                            if message.role == "tool" {
                                blocks.extend(message_row(&message, scrollback_columns, color));
                            } else {
                                blocks.extend(message_block(&message, scrollback_columns, color));
                            }
                        }
                        TurnMsg::Approve {
                            tool,
                            detail,
                            reply,
                        } => {
                            pending_approval = Some((tool, detail, reply));
                            break;
                        }
                        TurnMsg::AskUser {
                            question,
                            options,
                            reply,
                        } => {
                            pending_question = Some((question, options, reply));
                            break;
                        }
                    }
                }
                if !blocks.is_empty() {
                    renderer.flush(&blocks, &[])?;
                }
                if let Some((tool, detail, reply)) = pending_approval {
                    let decision = prompt_tool_approval(
                        renderer, &streamed, &tool, &detail, columns, color, input,
                    )?;
                    let _ = reply.send(decision);
                    continue;
                }
                if let Some((question, options, reply)) = pending_question {
                    let answer = prompt_user_question(
                        renderer, &streamed, &question, &options, columns, color, input,
                    )?;
                    let _ = reply.send(answer);
                    continue;
                }
                let cancelling = cancel.load(Ordering::Relaxed);
                let mut live = build_live(
                    &reasoning, &streamed, &note, frame, cancelling, columns, color,
                );
                let mut composer = busy_editor_lines(editor, queue, columns, color);
                let cursor_rows_below = if composer.len() > 1 {
                    place_editor_cursor(
                        &mut composer[1..],
                        editor.text(),
                        editor.cursor(),
                        columns,
                        0,
                    )
                } else {
                    0
                };
                live.extend(composer);
                renderer.flush_with_cursor(&[], &live, cursor_rows_below)?;
                frame = frame.wrapping_add(1);

                match input.read_or_message(&mut rx)? {
                    InputOrMessage::Message(Some(message)) => pending = Some(message),
                    InputOrMessage::Message(None) => break,
                    InputOrMessage::Input(event) => match event {
                        Event::Paste(text) => editor.paste(&text),
                        Event::Key(key)
                            if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
                        {
                            let is_ctrl_c = key.code == KeyCode::Char('c')
                                && key.modifiers.contains(KeyModifiers::CONTROL);
                            if key.code == KeyCode::Esc || is_ctrl_c {
                                cancel.store(true, Ordering::Relaxed);
                                crate::tool_runtime::runtime_ops::announce_cancellation();
                            } else if key.code == KeyCode::Up
                                && key.modifiers.contains(KeyModifiers::ALT)
                            {
                                if let Some(recalled) = queue.recall_last() {
                                    editor.set_text(recalled.text);
                                }
                            } else if key.code == KeyCode::Enter
                                && key.modifiers.contains(KeyModifiers::ALT)
                            {
                                editor.apply(EditorAction::InsertNewline);
                            } else if let Some(mut action) = queue.action_for(key) {
                                let text = editor.take();
                                if action == DeliveryAction::Steer && !steering_available {
                                    action = DeliveryAction::FollowUp;
                                    note = "Steering unavailable; queued as follow-up".into();
                                }
                                if let Err(error) = queue.push(text, action) {
                                    note = error.to_string();
                                }
                            } else {
                                editor.handle_key(key);
                            }
                        }
                        Event::Resize(_, _) => {}
                        _ => {}
                    },
                }
            }

            let mut blocks = Vec::new();
            commit_reasoning(&mut reasoning, &mut blocks, scrollback_columns, color);
            if !blocks.is_empty() {
                renderer.flush(&blocks, &[])?;
            }
            Ok(())
        })();
        if render_result.is_err() {
            cancel.store(true, Ordering::Relaxed);
            crate::tool_runtime::runtime_ops::announce_cancellation();
        }
        // Drop queued reply senders before joining a worker blocked on approval.
        drop(rx);
        let outcome = worker
            .join()
            .unwrap_or_else(|_| Err("Turn thread panicked.".into()));
        render_result?;
        Ok(outcome)
    })?;

    // Collapse the live region; the caller commits the finalized result.
    renderer.flush(&[], &[])?;
    Ok((outcome, tools_used))
}
