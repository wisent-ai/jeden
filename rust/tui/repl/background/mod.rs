//! Running one turn on a worker thread while the terminal keeps drawing.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};

use crate::agent::TraceEvent;
use crate::tui::render::{busy_editor_lines, place_editor_cursor};
use crate::tui::{
    default_columns, stdout_supports_color, CommandOutcome, DeliveryAction, EditorAction,
    EditorState, FollowUpQueue, TurnCtx,
};

use super::questions::prompt_user_question;
use super::{message_block, ReplRenderer};

mod events;
mod live;

use events::{prompt_tool_approval, trace_message, PendingQuestion, TurnMsg};
use live::{build_live, commit_reasoning};

/// Run a background turn on a worker thread while sweeping a skeleton bar and
/// draining live progress. Esc / Ctrl-C set the shared cancel flag, which the
/// agent loop polls between steps. Returns the handler's result.
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
) -> io::Result<(Result<CommandOutcome, String>, Vec<String>)>
where
    H: Fn(&str, &TurnCtx) -> Result<CommandOutcome, String> + Sync,
{
    let cancel = Arc::new(AtomicBool::new(false));
    // Note = the status line beside the skeleton; Delta = a live assistant text chunk.
    let (tx, rx) = mpsc::channel::<TurnMsg>();
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
    let columns = default_columns();
    let color = stdout_supports_color();
    // Committed blocks join the scrollback the main loop writes, at its width.
    let scrollback_columns = crossterm::terminal::size()
        .map(|(width, _)| usize::from(width).max(1))
        .unwrap_or(columns)
        .min(112);

    let outcome = thread::scope(|scope| -> io::Result<Result<CommandOutcome, String>> {
        let worker_cancel = cancel.clone();
        let note_tx = tx.clone();
        let delta_tx = tx.clone();
        let trace_tx = tx.clone();
        let approve_tx = tx.clone();
        let ask_tx = tx.clone();
        let worker = scope.spawn(move || {
            let progress = move |message: &str| {
                let _ = note_tx.send(TurnMsg::Note(message.to_string()));
            };
            let stream = move |piece: &str| {
                let _ = delta_tx.send(TurnMsg::Delta(piece.to_string()));
            };
            let trace = move |event: &TraceEvent<'_>| {
                let message = match *event {
                    TraceEvent::Reasoning { text } => TurnMsg::Reasoning(text.to_string()),
                    _ => match trace_message(event) {
                        Some(message) => TurnMsg::Trace(message),
                        None => return,
                    },
                };
                let _ = trace_tx.send(message);
            };
            let approve = move |tool: &str, detail: &str| -> bool {
                let (reply, answer) = mpsc::channel::<bool>();
                if approve_tx
                    .send(TurnMsg::Approve {
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
                    .send(TurnMsg::AskUser {
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

        loop {
            let mut pending_approval: Option<(String, String, mpsc::Sender<bool>)> = None;
            let mut pending_question: Option<PendingQuestion> = None;
            let mut blocks = Vec::new();
            while let Ok(message) = rx.try_recv() {
                match message {
                    TurnMsg::Note(m) => {
                        record_tool(&m, &mut tools_used);
                        note = m;
                    }
                    TurnMsg::Delta(p) => {
                        commit_reasoning(&mut reasoning, &mut blocks, scrollback_columns, color);
                        streamed.push_str(&p);
                    }
                    TurnMsg::Reasoning(p) => {
                        reasoning.push_str(&p);
                    }
                    TurnMsg::Trace(message) => {
                        commit_reasoning(&mut reasoning, &mut blocks, scrollback_columns, color);
                        blocks.extend(message_block(&message, scrollback_columns, color));
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
                let decision =
                    prompt_tool_approval(renderer, &streamed, &tool, &detail, columns, color)?;
                let _ = reply.send(decision);
                continue;
            }
            if let Some((question, options, reply)) = pending_question {
                let answer =
                    prompt_user_question(renderer, &streamed, &question, &options, columns, color)?;
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

            if worker.is_finished() {
                break;
            }
            if event::poll(Duration::from_millis(120))? {
                match event::read()? {
                    Event::Paste(text) => editor.paste(&text),
                    Event::Key(key)
                        if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
                    {
                        let is_ctrl_c = key.code == KeyCode::Char('c')
                            && key.modifiers.contains(KeyModifiers::CONTROL);
                        if key.code == KeyCode::Esc || is_ctrl_c {
                            cancel.store(true, Ordering::Relaxed);
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
                        if let Some(error) = editor.take_error() {
                            note = error.to_string();
                        }
                    }
                    Event::Resize(_, _) => {}
                    _ => {}
                }
            }
        }

        let mut blocks = Vec::new();
        while let Ok(message) = rx.try_recv() {
            match message {
                TurnMsg::Note(m) => {
                    record_tool(&m, &mut tools_used);
                    note = m;
                }
                TurnMsg::Delta(p) => {
                    commit_reasoning(&mut reasoning, &mut blocks, scrollback_columns, color);
                    streamed.push_str(&p);
                }
                TurnMsg::Reasoning(p) => {
                    reasoning.push_str(&p);
                }
                TurnMsg::Trace(message) => {
                    commit_reasoning(&mut reasoning, &mut blocks, scrollback_columns, color);
                    blocks.extend(message_block(&message, scrollback_columns, color));
                }
                TurnMsg::Approve { reply, .. } => {
                    let _ = reply.send(false);
                }
                TurnMsg::AskUser { reply, .. } => {
                    let _ = reply.send(Err("Question channel closed".into()));
                }
            }
        }
        commit_reasoning(&mut reasoning, &mut blocks, scrollback_columns, color);
        if !blocks.is_empty() {
            renderer.flush(&blocks, &[])?;
        }
        let _ = (note, streamed);
        Ok(worker
            .join()
            .unwrap_or_else(|_| Err("Turn thread panicked.".into())))
    })?;

    // Collapse the live region; the caller commits the finalized result.
    renderer.flush(&[], &[])?;
    Ok((outcome, tools_used))
}
