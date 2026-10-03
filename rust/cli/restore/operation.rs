//! One restore run: select the sessions the operator wrote in since an
//! instant, compare them with the sessions running now, reopen each one that
//! is stopped, and end by looking again, so every selected session is
//! reported as running, reopened or not running with its reason.

use super::launch::{self, Outcome};
use super::select::{self, Selected};
use super::{clock, Request};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;

const ALREADY_RUNNING: &str = "already_running";
const REOPENED: &str = "reopened";
const WOULD_REOPEN: &str = "would_reopen";
const REFUSED: &str = "refused";
const STOPPED: &str = "stopped";

pub(super) fn restore(request: &Request, announce: bool) -> Result<Value, String> {
    let since = clock::since(&request.since)?;
    // lsof names a held file by its real path, so the locks it is asked
    // about must be real paths too, or a running session behind a symlink
    // would read as stopped and be opened twice.
    let root = std::fs::canonicalize(&request.root)
        .map_err(|error| format!("cannot read the OMP session root {}: {error}", request.root.display()))?;
    let scan = select::scan(&root, since)?;
    let locks: Vec<PathBuf> = scan
        .sessions
        .iter()
        .map(|session| holders::owner_lock(&session.transcript))
        .collect();
    let before = holders::holders(&locks)?;

    let stopped: Vec<usize> = (0..scan.sessions.len())
        .filter(|index| !before.contains_key(&locks[*index]))
        .collect();
    let run = if request.dry_run || stopped.is_empty() {
        None
    } else {
        Some(launch::run_directory()?)
    };
    let jeden = std::env::current_exe()
        .map_err(|error| format!("cannot name this jeden executable for the new windows: {error}"))?;

    // What happened to each stopped session before the final look: the
    // window it was given, or why it got none.
    let mut windows: HashMap<usize, String> = HashMap::new();
    let mut refusals: HashMap<usize, String> = HashMap::new();
    if let Some(run) = &run {
        for index in &stopped {
            match launch::open_window(&jeden, &scan.sessions[*index].transcript, run) {
                Ok(tty) => {
                    windows.insert(*index, tty);
                }
                Err(error) => {
                    refusals.insert(*index, error);
                }
            }
        }
    }
    let opened_ids: Vec<String> = stopped
        .iter()
        .filter(|index| windows.contains_key(*index))
        .map(|index| scan.sessions[*index].id.clone())
        .collect();
    let outcomes = match (&run, opened_ids.is_empty()) {
        (Some(run), false) => launch::wait_started(run, &opened_ids, announce)?,
        _ => HashMap::new(),
    };

    // The closing check: who holds each lock now.
    let after = holders::holders(&locks)?;
    let mut rows = Vec::new();
    for (index, session) in scan.sessions.iter().enumerate() {
        let lock = &locks[index];
        let row = if let Some(pids) = before.get(lock) {
            match after.get(lock) {
                Some(pids) => row(session, ALREADY_RUNNING, json!({"pids": pids})),
                None => row(session, STOPPED, json!({
                    "pids": pids,
                    "reason": "it was running when restore began and its process has ended since",
                })),
            }
        } else if request.dry_run {
            row(session, WOULD_REOPEN, json!({}))
        } else if let Some(reason) = refusals.get(&index) {
            row(session, REFUSED, json!({"reason": reason}))
        } else {
            reopened(session, lock, &after, windows.get(&index), outcomes.get(&session.id))
        };
        rows.push(row);
    }

    let unreadable: Vec<Value> = scan
        .unreadable
        .iter()
        .map(|entry| json!({"transcript": entry.transcript, "error": entry.error}))
        .collect();
    let count = |state: &str| rows.iter().filter(|row| row["state"] == state).count();
    let not_running = count(REFUSED) + count(STOPPED);
    Ok(json!({
        "since": request.since,
        "sinceUtc": clock::format_utc(since),
        "root": root,
        "dryRun": request.dry_run,
        "run": run,
        "sessions": rows,
        "unreadable": unreadable,
        "counts": {
            "selected": rows.len(),
            "alreadyRunning": count(ALREADY_RUNNING),
            "reopened": count(REOPENED),
            "wouldReopen": count(WOULD_REOPEN),
            "refused": count(REFUSED),
            "stopped": count(STOPPED),
            "unreadable": unreadable.len(),
        },
        "complete": not_running == 0 && unreadable.is_empty(),
    }))
}

/// A session given a window: reopened when its process started and is
/// still there (or something now holds its lock), otherwise not running
/// with what the window recorded.
fn reopened(
    session: &Selected,
    lock: &PathBuf,
    after: &HashMap<PathBuf, Vec<u32>>,
    tty: Option<&String>,
    outcome: Option<&Outcome>,
) -> Value {
    match outcome {
        Some(Outcome::Started(pid)) if after.contains_key(lock) || holders::alive(*pid) => {
            row(session, REOPENED, json!({"pids": [pid], "tty": tty}))
        }
        Some(Outcome::Started(pid)) => row(session, STOPPED, json!({
            "tty": tty,
            "reason": format!("its process {pid} ended right after it started; the Terminal window {} shows why", tty.map(String::as_str).unwrap_or("it opened")),
        })),
        Some(Outcome::Failed(reason)) => row(session, REFUSED, json!({"tty": tty, "reason": reason})),
        None => row(session, REFUSED, json!({"tty": tty, "reason": "its window recorded no start"})),
    }
}

fn row(session: &Selected, state: &str, detail: Value) -> Value {
    let mut row = json!({
        "sessionId": session.id,
        "title": session.title,
        "cwd": session.cwd,
        "transcript": session.transcript,
        "lastOperatorMessage": clock::format_utc(session.last_operator_message),
        "state": state,
    });
    if let (Some(row), Some(detail)) = (row.as_object_mut(), detail.as_object()) {
        for (key, value) in detail {
            row.insert(key.clone(), value.clone());
        }
    }
    row
}

/// Which sessions run now. OMP keeps `.<transcript>.owner.lock` open for the
/// whole life of a session process and leaves the file behind when the
/// process dies, so the file proves nothing: a process holding it open is
/// what says the session runs.
mod holders {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    pub(super) fn owner_lock(transcript: &Path) -> PathBuf {
        let name = transcript
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        transcript.with_file_name(format!(".{name}.owner.lock"))
    }

    /// The processes holding each lock open, read in one `lsof` call. A lock
    /// with no holder is absent from the map.
    pub(super) fn holders(locks: &[PathBuf]) -> Result<HashMap<PathBuf, Vec<u32>>, String> {
        let present: Vec<&PathBuf> = locks.iter().filter(|lock| lock.exists()).collect();
        let mut held: HashMap<PathBuf, Vec<u32>> = HashMap::new();
        if present.is_empty() {
            return Ok(held);
        }
        // `-F pn` answers one `p<pid>` line per process followed by one
        // `n<name>` line per named file it holds.
        let output = Command::new("lsof")
            .args(["-F", "pn", "--"])
            .args(&present)
            .output()
            .map_err(|error| format!("cannot run lsof to see which sessions run: {error}"))?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        // lsof exits 1 whenever one named file is open nowhere, so its status
        // alone is no failure; a failure is an exit with no answer and a reason.
        if !output.status.success() && stdout.trim().is_empty() && !stderr.trim().is_empty() {
            return Err(format!(
                "lsof could not read which processes hold the session locks: {}",
                stderr.trim()
            ));
        }
        let mut pid = None;
        for line in stdout.lines() {
            if let Some(number) = line.strip_prefix('p') {
                pid = number.parse::<u32>().ok();
            } else if let (Some(name), Some(pid)) = (line.strip_prefix('n'), pid) {
                let holders = held.entry(PathBuf::from(name)).or_default();
                if !holders.contains(&pid) {
                    holders.push(pid);
                }
            }
        }
        Ok(held)
    }

    /// Whether process `pid` still exists.
    pub(super) fn alive(pid: u32) -> bool {
        let Ok(pid) = libc::pid_t::try_from(pid) else {
            return false;
        };
        unsafe { libc::kill(pid, 0) == 0 }
    }
}
