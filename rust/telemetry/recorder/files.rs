//! Putting telemetry on disk as a file only its owner can read, and taking
//! records back out of it when they expire.
//!
//! Split out of `telemetry/recorder.rs`, which had grown past the module line
//! cap.

use super::super::schema::TelemetryEnvelope;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) fn append_envelope(path: &Path, envelope: &TelemetryEnvelope) -> Result<(), ()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|_| ())?;
    }
    let file = private_append_file(path)?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer(&mut writer, envelope).map_err(|_| ())?;
    writer.write_all(b"\n").map_err(|_| ())?;
    writer.flush().map_err(|_| ())
}

#[derive(Default)]
pub(super) struct FileFilterReport {
    pub(super) removed: u64,
    pub(super) malformed: u64,
}

pub(super) fn rewrite_filtered(
    path: &Path,
    keep: impl Fn(&TelemetryEnvelope) -> bool,
) -> Result<FileFilterReport, ()> {
    if !path.exists() {
        return Ok(FileFilterReport::default());
    }
    let input = fs::File::open(path).map_err(|_| ())?;
    let temporary = path.with_extension("telemetry.tmp");
    let output = private_replace_file(&temporary)?;
    let mut writer = BufWriter::new(output);
    let mut report = FileFilterReport::default();
    for line in BufReader::new(input).lines() {
        let line = line.map_err(|_| ())?;
        let envelope = match serde_json::from_str::<TelemetryEnvelope>(&line) {
            Ok(envelope) => envelope,
            Err(_) => {
                report.malformed += 1;
                continue;
            }
        };
        if keep(&envelope) {
            serde_json::to_writer(&mut writer, &envelope).map_err(|_| ())?;
            writer.write_all(b"\n").map_err(|_| ())?;
        } else {
            report.removed += 1;
        }
    }
    writer.flush().map_err(|_| ())?;
    writer.get_ref().sync_all().map_err(|_| ())?;
    fs::rename(&temporary, path).map_err(|_| ())?;
    Ok(report)
}

#[cfg(unix)]
fn private_append_file(path: &Path) -> Result<fs::File, ()> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
        .map_err(|_| ())?;
    file.set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(|_| ())?;
    Ok(file)
}

#[cfg(not(unix))]
fn private_append_file(path: &Path) -> Result<fs::File, ()> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|_| ())
}

#[cfg(unix)]
fn private_replace_file(path: &Path) -> Result<fs::File, ()> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .map_err(|_| ())
}

#[cfg(not(unix))]
fn private_replace_file(path: &Path) -> Result<fs::File, ()> {
    OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)
        .map_err(|_| ())
}

pub(super) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}
