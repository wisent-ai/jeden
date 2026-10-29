//! `jeden import <path>... [--refresh] [--json]`: the command's own options.

use std::path::PathBuf;

use super::import_paths;

const USAGE: &str = "Usage: jeden import <path>... [--refresh] [--json]";

/// `--help` or `-h` anywhere prints the usage and imports nothing; any other
/// option but `--refresh` is refused naming it, never read as a transcript
/// path.
pub(crate) fn command(args: &crate::Args) -> Result<String, String> {
    if args
        .positionals
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
    {
        return Ok(USAGE.to_string());
    }
    if let Some(option) = args
        .positionals
        .iter()
        .find(|argument| argument.starts_with('-') && *argument != "--refresh")
    {
        return Err(crate::cli::invocation::refusal::usage(format!(
            "unknown option for jeden import: {option}\n{USAGE}"
        )));
    }
    let refresh = args
        .positionals
        .iter()
        .any(|argument| argument == "--refresh");
    let paths: Vec<PathBuf> = args
        .positionals
        .iter()
        .filter(|argument| *argument != "--refresh")
        .map(PathBuf::from)
        .collect();
    if paths.is_empty() {
        return Err(crate::cli::invocation::refusal::usage(USAGE));
    }
    let result = import_paths(&paths, refresh)?;
    serde_json::to_string_pretty(&result)
        .map_err(|error| format!("the import report cannot be written as JSON: {error}"))
}
