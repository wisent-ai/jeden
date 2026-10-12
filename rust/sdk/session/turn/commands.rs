//! Session controls operate on the SDK's active conversation, not a model's interpretation.
use crate::agent::{self, Conversation, RunHooks};
use crate::Args;

pub(super) fn run(
    conversation: &mut Conversation,
    args: &Args,
    prompt: &str,
    continuing: bool,
    hooks: &mut RunHooks,
) -> Result<String, String> {
    if continuing {
        return conversation.continue_work(args, hooks);
    }
    if args.model_only {
        return conversation.run_turn(args, prompt, &[], hooks);
    }
    let input = prompt.trim();
    let command = input
        .split_whitespace()
        .next()
        .ok_or("session prompt is empty")?;
    let rest = input[command.len()..].trim_start();
    match command {
        "/compact" => conversation.compact(args, rest, hooks),
        "/handoff" => conversation.handoff(args, rest, hooks),
        "/move" => {
            let resolved = conversation.move_to(rest)?;
            Ok(format!("Working directory moved to {}. Tools, git status, and file commands now resolve there.", resolved.display()))
        }
        "/checkpoint" if rest.trim() == "list" => conversation.list_checkpoints(),
        "/checkpoint" => conversation.checkpoint(rest),
        "/rewind" => conversation.rewind(rest),
        "/new" | "/fresh" => {
            conversation.reset(&args.cwd)?;
            Ok("Started a fresh conversation; prior turns cleared.".into())
        }
        "/fork" => {
            let path = conversation.fork(&args.cwd)?;
            Ok(format!(
                "Forked into a new session at {}; the current context continues there.",
                path.display()
            ))
        }
        "/branch" => {
            let path = conversation.branch(&args.cwd)?;
            let id = agent::record_branch(&args.cwd, rest.trim(), &path)?;
            Ok(format!(
                "Branch {id} created at {}; the current context continues on this branch. List with /tree, switch with /resume {}.",
                path.display(), path.display()
            ))
        }
        "/todo" => {
            let mut parts = shell_words::split(rest).map_err(|error| error.to_string())?;
            if parts.first().map(String::as_str) == Some("continue") {
                conversation.continue_work(args, hooks)
            } else {
                parts.extend([
                    "--session".into(),
                    conversation.session_path().display().to_string(),
                ]);
                crate::completion::cli::execute(&args.cwd, &parts, false, Some(args))
            }
        }
        _ => conversation.run_turn(args, prompt, &[], hooks),
    }
}
