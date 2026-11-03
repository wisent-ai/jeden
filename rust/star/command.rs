//! `jeden star` and `/star`: read, dismiss and reset the GitHub star request.
//! The verbs, their text and JSON, and their refusals are the shared
//! runtime's `ProductStarRequest::command`; an unknown verb or stray word is
//! a usage refusal here.

/// One verb of the command, as the runtime answers it.
pub(crate) fn execute(words: &[String], json: bool) -> Result<String, String> {
    match super::request()?.command(words, json) {
        Err(refusal) if refusal.starts_with("star: ") => {
            Err(crate::cli::invocation::refusal::usage(refusal))
        }
        answered => answered,
    }
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
