//! Running one submitted prompt and everything queued behind it.
//!
//! Split out of `tui/repl/loops.rs`, which had grown past the module line cap.

use std::io;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};

use crate::tui::{
    stdout_supports_color, Attachment, CommandOutcome, EditorState, FollowUpQueue, Message,
    PickerState, PromptStatus, RegistryUiRuntime, TurnCtx, TurnKind, UiFeature, UiRuntimeAdapter,
};

use super::super::background::run_background_turn;
use super::super::{apply_turn_result, message_block, ReplRenderer};
use super::plain::terminal_dimensions;

/// Returns whether the session should end.
#[allow(clippy::too_many_arguments)]
pub(super) fn run_turn_chain<S, C, H>(
    mut active_prompt: String,
    mut active_attachments: Vec<Attachment>,
    mut active_from_view: bool,
    status_provider: &mut S,
    classify: &mut C,
    handler: &H,
    renderer: &mut ReplRenderer,
    messages: &mut Vec<Message>,
    committed: &mut usize,
    editor: &mut EditorState,
    follow_ups: &mut FollowUpQueue,
    picker: &mut Option<PickerState>,
    view: &mut Option<Message>,
    runtime: &RegistryUiRuntime,
) -> io::Result<bool>
where
    S: FnMut() -> PromptStatus,
    C: FnMut(&str) -> TurnKind,
    H: Fn(&str, &TurnCtx) -> Result<CommandOutcome, String> + Sync,
{
                loop {
                    match classify(&active_prompt) {
                        TurnKind::Foreground => {
                            crossterm::execute!(io::stdout(), DisableBracketedPaste)?;
                            disable_raw_mode()?;
                            let ctx = TurnCtx {
                                cancel: Arc::new(AtomicBool::new(false)),
                                interactive: true,
                                from_view: active_from_view,
                                attachments: &active_attachments,
                                progress: &|_| {},
                                stream: &|_| {},
                                trace: &|_| {},
                                ask_user: None,
                                approve: &|_, _| false,
                            };
                            let result = handler(&active_prompt, &ctx);
                            enable_raw_mode()?;
                            crossterm::execute!(io::stdout(), EnableBracketedPaste)?;
                            renderer.reset();
                            if apply_turn_result(messages, &active_prompt, result, picker, view) {
                                return Ok(true);
                            }
                        }
                        TurnKind::Background => {
                            let steering_available = runtime
                                .availability(
                                    Path::new(&status_provider().cwd),
                                    UiFeature::Steering,
                                )
                                .available;
                            let (result, tools_used) = run_background_turn(
                                renderer,
                                handler,
                                &active_prompt,
                                active_from_view,
                                &active_attachments,
                                editor,
                                follow_ups,
                                steering_available,
                            )?;
                            if !tools_used.is_empty() {
                                messages.push(Message::new(
                                    "system",
                                    format!("tools: {}", tools_used.join(", ")),
                                ));
                            }
                            if apply_turn_result(messages, &active_prompt, result, picker, view) {
                                return Ok(true);
                            }
                        }
                    }

                    let Some(queued) = follow_ups.pop_next() else {
                        break;
                    };
                    active_prompt = queued.text;
                    active_attachments = Vec::new();
                    active_from_view = false;
                    editor.push_history(active_prompt.clone());
                    messages.push(Message::new("user", active_prompt.clone()));
                    let (columns, _) = terminal_dimensions();
                    let color = stdout_supports_color();
                    let mut blocks = Vec::new();
                    for message in &messages[*committed..] {
                        blocks.extend(message_block(message, columns.min(112), color));
                    }
                    *committed = messages.len();
                    renderer.flush(&blocks, &[])?;
                }
    Ok(false)
}
