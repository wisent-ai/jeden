//! Reopening one session: a new Terminal window whose shell is replaced by
//! `jeden restore open`, which records that it started and then becomes
//! `omp --resume` for that session under the same process id.
//!
//! The restore run learns that each window started from the record the
//! window writes into the run directory, told by the kernel's directory
//! watch rather than read on a clock.

use super::select;
use std::collections::HashMap;
use std::fs;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

pub(super) enum Outcome {
    Started(u32),
    Failed(String),
}

const STARTED: &str = "started";
const FAILED: &str = "failed";

/// A fresh directory for one restore run's start records, under
/// `~/.jeden/restore/`.
pub(super) fn run_directory() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_millis();
    let run = PathBuf::from(home)
        .join(".jeden/restore")
        .join(format!("{stamp}-{}", std::process::id()));
    fs::create_dir_all(&run).map_err(|error| {
        format!(
            "cannot create the restore run directory {}: {error}",
            run.display()
        )
    })?;
    Ok(run)
}

/// Opens one Terminal window running `exec jeden restore open <transcript>
/// --run <run>` and answers the window's terminal device.
pub(super) fn open_window(jeden: &Path, transcript: &Path, run: &Path) -> Result<String, String> {
    if !cfg!(target_os = "macos") {
        return Err(
            "reopening a session opens a macOS Terminal window; this host is not macOS".into(),
        );
    }
    let command = format!(
        "exec {} restore open {} --run {}",
        quote(jeden),
        quote(transcript),
        quote(run)
    );
    // The command travels as an argument, never spliced into the script, so
    // no path needs AppleScript escaping.
    let output = Command::new("osascript")
        .args([
            "-e",
            "on run argv",
            "-e",
            "tell application \"Terminal\"",
            "-e",
            "set opened to do script (item 1 of argv)",
            "-e",
            "return tty of opened",
            "-e",
            "end tell",
            "-e",
            "end run",
        ])
        .arg(&command)
        .output()
        .map_err(|error| format!("cannot run osascript to open a Terminal window: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Terminal did not open a window: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// `jeden restore open <transcript> [--run <dir>]`: become `omp --resume`
/// for the transcript's session, in its workspace. With `--run`, first
/// record this process id there so the restore run knows the window started.
/// Everything `exec` could refuse is checked before that record is written.
pub(super) fn open(transcript: &Path, run: Option<&Path>) -> Result<String, String> {
    let result = prepare(transcript).and_then(|(header, mut command)| {
        if let Some(run) = run {
            record(run, &header.id, STARTED, &std::process::id().to_string())?;
        }
        let error = command.exec();
        Err(format!(
            "cannot start omp for session {}: {error}",
            header.id
        ))
    });
    if let (Err(error), Some(run)) = (&result, run) {
        let id = select::header(transcript)
            .map(|header| header.id)
            .unwrap_or_else(|_| {
                transcript
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned())
                    .unwrap_or_default()
            });
        record(run, &id, FAILED, error)?;
    }
    result
}

fn prepare(transcript: &Path) -> Result<(select::Header, Command), String> {
    let header = select::header(transcript)?;
    let omp = on_path("omp")
        .ok_or("omp is not on PATH in the new window, so the session cannot resume")?;
    let workspace = Path::new(&header.cwd);
    if !workspace.is_dir() {
        return Err(format!(
            "the session's workspace {} is gone",
            workspace.display()
        ));
    }
    let mut command = Command::new(omp);
    command
        .arg(format!("--resume={}", header.id))
        .current_dir(workspace);
    // OMP refuses to run in the home directory unless told to; a session
    // whose workspace is home was started that way.
    if std::env::var_os("HOME").is_some_and(|home| Path::new(&home) == workspace) {
        command.arg("--allow-home");
    }
    Ok((header, command))
}

/// Holds until every window in `ids` has recorded that it started or failed.
/// The watch is armed before the first look, so a record written between
/// the look and the wait still wakes it.
pub(super) fn wait_started(
    run: &Path,
    ids: &[String],
    announce: bool,
) -> Result<HashMap<String, Outcome>, String> {
    let watch = crate::task_runtime::watch::watch(run).map_err(|error| error.to_string())?;
    let mut announced = false;
    loop {
        let mut outcomes = HashMap::new();
        for id in ids {
            if let Some(outcome) = outcome(run, id)? {
                outcomes.insert(id.clone(), outcome);
            }
        }
        if outcomes.len() == ids.len() {
            return Ok(outcomes);
        }
        if announce && !announced {
            let waiting: Vec<&str> = ids
                .iter()
                .filter(|id| !outcomes.contains_key(*id))
                .map(String::as_str)
                .collect();
            eprintln!(
                "waiting for {} Terminal window(s) to start their session: {}",
                waiting.len(),
                waiting.join(", ")
            );
            announced = true;
        }
        watch.wait().map_err(|error| {
            format!(
                "cannot wait for the Terminal windows in {}: {error}",
                run.display()
            )
        })?;
    }
}

fn outcome(run: &Path, id: &str) -> Result<Option<Outcome>, String> {
    if let Some(text) = read_record(run, id, FAILED)? {
        return Ok(Some(Outcome::Failed(text)));
    }
    match read_record(run, id, STARTED)? {
        Some(text) => text
            .trim()
            .parse()
            .map(|pid| Some(Outcome::Started(pid)))
            .map_err(|_| format!("the start record of session {id} holds no process id: {text:?}")),
        None => Ok(None),
    }
}

fn read_record(run: &Path, id: &str, kind: &str) -> Result<Option<String>, String> {
    let path = run.join(format!("{id}.{kind}"));
    match fs::read_to_string(&path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("cannot read {}: {error}", path.display())),
    }
}

/// Written whole and renamed into place, so the watcher never reads half.
fn record(run: &Path, id: &str, kind: &str, text: &str) -> Result<(), String> {
    let path = run.join(format!("{id}.{kind}"));
    let partial = run.join(format!(".{id}.{kind}.partial"));
    fs::write(&partial, text)
        .and_then(|_| fs::rename(&partial, &path))
        .map_err(|error| format!("cannot record {}: {error}", path.display()))
}

fn on_path(program: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

fn quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}
