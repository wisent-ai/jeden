//! `jeden star` and `/star`: read, dismiss and reset the GitHub star request.

use super::{dismiss, reset, status, status_text, Status};

const USAGE: &str = "usage: jeden star [status|dismiss|reset] [--json]";

fn render(status: &Status, json: bool) -> Result<String, String> {
    if json {
        serde_json::to_string(status)
            .map(|text| text + "\n")
            .map_err(|error| format!("the star status cannot be written as JSON: {error}"))
    } else {
        Ok(status_text(status))
    }
}

/// One verb of the command; an unknown verb or a stray argument is a usage
/// refusal naming every verb.
pub(crate) fn execute(words: &[String], json: bool) -> Result<String, String> {
    let status = match words {
        [] => status(),
        [verb] if verb == "status" => status(),
        [verb] if verb == "dismiss" => dismiss(),
        [verb] if verb == "reset" => reset(),
        [verb] => {
            return Err(crate::cli::invocation::refusal::usage(format!(
                "star: unknown action {verb:?}; {USAGE}"
            )))
        }
        [_, unexpected, ..] => {
            return Err(crate::cli::invocation::refusal::usage(format!(
                "star: unexpected argument {unexpected:?}; {USAGE}"
            )))
        }
    }?;
    render(&status, json)
}

/// `jeden star …`
pub(crate) fn command(args: &crate::Args) -> Result<String, String> {
    execute(&args.positionals, args.json)
}

/// `/star …` in a session, the same verbs and `--json`.
pub(crate) fn slash(args: &str) -> Result<String, String> {
    let mut words = crate::slash::common::split_args(args);
    let json = words.iter().any(|word| word == "--json");
    words.retain(|word| word != "--json");
    execute(&words, json)
}
