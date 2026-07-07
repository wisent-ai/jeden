use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};

use super::{message_block, spinner_glyph, ReplRenderer};
use super::super::{default_columns, stdout_supports_color, Message, TurnCtx};

/// Worker→render-loop message during a background turn.
enum TurnMsg {
    /// Spinner status line ("thinking…", "tool: read_file").
    Note(String),
    /// A chunk of live assistant text.
    Delta(String),
    /// Approval request for a gated tool; the main loop prompts and replies.
    Approve { tool: String, detail: String, reply: mpsc::Sender<bool> },
}

/// Prompt the user (in the live region) to approve one gated tool. Blocks on a
/// keystroke: `y` allows, anything else (incl. Esc) denies. Returns the choice.
fn prompt_tool_approval(renderer: &mut ReplRenderer, streamed: &str, tool: &str, detail: &str, columns: usize, color: bool) -> io::Result<bool> {
    let mut lines = Vec::new();
    if !streamed.trim().is_empty() {
        lines.extend(message_block(&Message::new("assistant", streamed.to_string()), columns, color));
    }
    let ask = if detail.trim().is_empty() {
        format!("Allow tool \"{}\" for this call? [y]es / [n]o", tool)
    } else {
        format!("Allow tool \"{}\" for this call? [y]es / [n]o\nReason: {}", tool, detail.trim())
    };
    lines.extend(message_block(&Message::new("system", ask), columns, color));
    renderer.flush(&[], &lines)?;
    loop {
        if event::poll(Duration::from_millis(250))? {
            if let Event::Key(key) = event::read()? {
                if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
                    continue;
                }
                return Ok(matches!(key.code, KeyCode::Char('y') | KeyCode::Char('Y')));
            }
        }
    }
}

/// Run a background turn on a worker thread while animating a spinner and
/// draining live progress. Esc / Ctrl-C set the shared cancel flag, which the
/// agent loop polls between steps. Returns the handler's result.
pub(super) fn run_background_turn<H>(
    renderer: &mut ReplRenderer,
    handler: &H,
    prompt: &str,
) -> io::Result<(Result<String, String>, Vec<String>)>
where
    H: Fn(&str, &TurnCtx) -> Result<String, String> + Sync,
{
    let cancel = Arc::new(AtomicBool::new(false));
    // Note = spinner status line; Delta = a live assistant text chunk.
    let (tx, rx) = mpsc::channel::<TurnMsg>();
    let mut note = String::from("working…");
    let mut streamed = String::new();
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
    let _ = ();
    let columns = default_columns();
    let color = stdout_supports_color();

    // Build the live region for a background turn: streamed assistant text (as it
    // arrives) above the spinner status line.
    let build_live = |streamed: &str, note: &str, frame: usize, cancelling: bool| -> Vec<String> {
        let mut lines = Vec::new();
        if !streamed.trim().is_empty() {
            lines.extend(message_block(&Message::new("assistant", streamed.to_string()), columns, color));
        }
        let label = if cancelling {
            format!("{} cancelling…", spinner_glyph(frame))
        } else {
            format!("{} {} · esc to cancel", spinner_glyph(frame), note)
        };
        lines.extend(message_block(&Message::new("system", label), columns, color));
        lines
    };

    let outcome = thread::scope(|scope| -> io::Result<Result<String, String>> {
        let worker_cancel = cancel.clone();
        let note_tx = tx.clone();
        let delta_tx = tx.clone();
        let approve_tx = tx.clone();
        let worker = scope.spawn(move || {
            let progress = move |message: &str| {
                let _ = note_tx.send(TurnMsg::Note(message.to_string()));
            };
            let stream = move |piece: &str| {
                let _ = delta_tx.send(TurnMsg::Delta(piece.to_string()));
            };
            let approve = move |tool: &str, detail: &str| -> bool {
                let (reply, answer) = mpsc::channel::<bool>();
                if approve_tx.send(TurnMsg::Approve { tool: tool.to_string(), detail: detail.to_string(), reply }).is_err() {
                    return false;
                }
                answer.recv().unwrap_or(false)
            };
            let ctx = TurnCtx { cancel: worker_cancel, interactive: false, progress: &progress, stream: &stream, approve: &approve };
            handler(prompt, &ctx)
        });
        drop(tx);

        loop {
            let mut pending_approval: Option<(String, String, mpsc::Sender<bool>)> = None;
            while let Ok(message) = rx.try_recv() {
                match message {
                    TurnMsg::Note(m) => { record_tool(&m, &mut tools_used); note = m; }
                    TurnMsg::Delta(p) => { streamed.push_str(&p); }
                    TurnMsg::Approve { tool, detail, reply } => { pending_approval = Some((tool, detail, reply)); break; }
                }
            }
            if let Some((tool, detail, reply)) = pending_approval {
                let decision = prompt_tool_approval(renderer, &streamed, &tool, &detail, columns, color)?;
                let _ = reply.send(decision);
                continue;
            }
            let cancelling = cancel.load(Ordering::Relaxed);
            let live = build_live(&streamed, &note, frame, cancelling);
            renderer.flush(&[], &live)?;
            frame = frame.wrapping_add(1);

            if worker.is_finished() {
                break;
            }
            if event::poll(Duration::from_millis(120))? {
                if let Event::Key(key) = event::read()? {
                    if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
                        let is_ctrl_c = key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL);
                        if key.code == KeyCode::Esc || is_ctrl_c {
                            cancel.store(true, Ordering::Relaxed);
                        }
                    }
                }
            }
        }

        while let Ok(message) = rx.try_recv() {
            match message {
                TurnMsg::Note(m) => { record_tool(&m, &mut tools_used); note = m; }
                TurnMsg::Delta(p) => { streamed.push_str(&p); }
                TurnMsg::Approve { reply, .. } => { let _ = reply.send(false); }
            }
        }
        let _ = (note, streamed);
        Ok(worker.join().unwrap_or_else(|_| Err("Turn thread panicked.".into())))
    })?;

    // Collapse the live region; the caller commits the finalized result.
    renderer.flush(&[], &[])?;
    Ok((outcome, tools_used))
}
