//! `jeden restore`: after the computer froze or the terminal host died,
//! reopen every agent session the operator wrote in since a given time —
//! only those, never older ones — and say for each selected session whether
//! it was already running, was reopened, or is not running and why.
//!
//! Sessions are OMP's (`~/.omp/agent/sessions`). Selection reads the
//! transcripts, the running check reads which processes hold each session's
//! owner lock, and a stopped session is reopened in a new macOS Terminal
//! window as `omp --resume=<id>` in its own workspace.

#[cfg(unix)]
mod clock;
#[cfg(unix)]
mod launch;
#[cfg(unix)]
mod operation;
#[cfg(unix)]
mod select;

use crate::cli::invocation::refusal;
use serde_json::Value;
use std::path::PathBuf;

const USAGE: &str = "Usage: jeden restore --since <today|YYYY-MM-DD|YYYY-MM-DDTHH:MM:SSZ> [--dry-run] [--sessions <dir>] [--json] | jeden restore open <transcript> [--run <dir>]";

#[cfg(not(unix))]
const UNSUPPORTED: &str = "jeden restore reads session locks with lsof and opens macOS Terminal windows; this host has neither";

pub(crate) struct Request {
    pub(crate) since: String,
    pub(crate) dry_run: bool,
    pub(crate) root: PathBuf,
}

/// Where OMP keeps its state: `~/.omp`.
fn omp_home() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join(".omp"))
        .ok_or_else(|| "HOME is not set, so the OMP state directory cannot be found".into())
}

/// Where OMP keeps its session transcripts.
pub(crate) fn default_root() -> Result<PathBuf, String> {
    Ok(omp_home()?.join("agent/sessions"))
}

/// Where OMP keeps one `<session id>.lock` per session, held open by the
/// process that owns the session. It does not move with `--sessions`.
#[cfg(unix)]
fn owner_locks() -> Result<PathBuf, String> {
    Ok(omp_home()?.join("run/session-owners"))
}

pub(crate) fn command(args: &crate::Args) -> Result<String, String> {
    match args.positionals.first().map(String::as_str) {
        Some("open") => open_command(&args.positionals[1..]),
        _ => restore_command(args),
    }
}

/// The operation both the command line and `session/restore` run. With
/// `announce`, the sessions still starting are named on stderr before the
/// run holds for them.
#[cfg(unix)]
pub(crate) fn restore(request: &Request, announce: bool) -> Result<Value, String> {
    operation::restore(request, announce)
}

#[cfg(not(unix))]
pub(crate) fn restore(_request: &Request, _announce: bool) -> Result<Value, String> {
    Err(UNSUPPORTED.into())
}

fn restore_command(args: &crate::Args) -> Result<String, String> {
    let mut since = None;
    let mut dry_run = false;
    let mut root = None;
    let mut words = args.positionals.iter();
    while let Some(word) = words.next() {
        match word.as_str() {
            "--since" => {
                since = Some(
                    words
                        .next()
                        .ok_or_else(|| refusal::usage("--since requires a value"))?
                        .clone(),
                )
            }
            "--sessions" => {
                let dir = words
                    .next()
                    .ok_or_else(|| refusal::usage("--sessions requires a directory"))?;
                root = Some(PathBuf::from(dir))
            }
            "--dry-run" => dry_run = true,
            other => {
                return Err(refusal::usage(format!(
                    "restore does not take {other:?}\n{USAGE}"
                )))
            }
        }
    }
    let since =
        since.ok_or_else(|| refusal::usage(format!("restore requires --since\n{USAGE}")))?;
    let root = match root {
        Some(root) => root,
        None => default_root()?,
    };
    let report = restore(
        &Request {
            since,
            dry_run,
            root,
        },
        true,
    )?;
    let text = if args.json {
        serde_json::to_string_pretty(&report).map_err(|error| error.to_string())? + "\n"
    } else {
        render(&report)
    };
    if report["complete"] == true {
        return Ok(text);
    }
    // The report is the answer either way; the exit status says that some
    // selected session is not running or could not be read.
    print!("{text}");
    let counts = &report["counts"];
    Err(format!(
        "{} refused, {} stopped and {} unreadable of {} selected sessions",
        counts["refused"], counts["stopped"], counts["unreadable"], counts["selected"]
    ))
}

fn open_command(words: &[String]) -> Result<String, String> {
    let mut transcript = None;
    let mut run = None;
    let mut words = words.iter();
    while let Some(word) = words.next() {
        match word.as_str() {
            "--run" => {
                let dir = words
                    .next()
                    .ok_or_else(|| refusal::usage("--run requires a directory"))?;
                run = Some(PathBuf::from(dir))
            }
            other if other.starts_with("--") => {
                return Err(refusal::usage(format!(
                    "restore open does not take {other:?}\n{USAGE}"
                )))
            }
            other if transcript.is_none() => transcript = Some(PathBuf::from(other)),
            other => {
                return Err(refusal::usage(format!(
                    "restore open takes one transcript, not also {other:?}"
                )))
            }
        }
    }
    let transcript = transcript
        .ok_or_else(|| refusal::usage(format!("restore open requires a transcript\n{USAGE}")))?;
    #[cfg(unix)]
    {
        launch::open(&transcript, run.as_deref())
    }
    #[cfg(not(unix))]
    {
        let _ = (transcript, run);
        Err(UNSUPPORTED.into())
    }
}

/// One line per selected session, then the counts.
fn render(report: &Value) -> String {
    let text_of = |value: &Value| value.as_str().unwrap_or_default().to_string();
    let mut text = format!(
        "Sessions with an operator message since {} ({}) under {}:\n",
        text_of(&report["sinceUtc"]),
        text_of(&report["since"]),
        text_of(&report["root"]),
    );
    for session in report["sessions"].as_array().into_iter().flatten() {
        let state = text_of(&session["state"]).replace('_', " ");
        let mut line = format!(
            "  {state:<16} {}  {}  {}  last message {}",
            text_of(&session["sessionId"]),
            text_of(&session["title"]),
            text_of(&session["cwd"]),
            text_of(&session["lastOperatorMessage"]),
        );
        if let Some(pids) = session["pids"].as_array() {
            let pids: Vec<String> = pids.iter().map(Value::to_string).collect();
            line.push_str(&format!("  pid {}", pids.join(",")));
        }
        if let Some(tty) = session["tty"].as_str() {
            line.push_str(&format!("  in {tty}"));
        }
        if let Some(reason) = session["reason"].as_str() {
            line.push_str(&format!("\n      {reason}"));
        }
        text.push_str(&line);
        text.push('\n');
    }
    for entry in report["unreadable"].as_array().into_iter().flatten() {
        text.push_str(&format!(
            "  unreadable       {}\n      {}\n",
            text_of(&entry["transcript"]),
            text_of(&entry["error"])
        ));
    }
    let counts = &report["counts"];
    text.push_str(&format!(
        "{} selected: {} already running, {} reopened, {} would reopen, {} refused, {} stopped, {} unreadable.\n",
        counts["selected"],
        counts["alreadyRunning"],
        counts["reopened"],
        counts["wouldReopen"],
        counts["refused"],
        counts["stopped"],
        counts["unreadable"]
    ));
    text
}
