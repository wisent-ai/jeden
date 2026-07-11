use std::collections::VecDeque;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, TryLockError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::export::{ExportStatus, OtlpExporter};
use super::schema::{PrivateId, TelemetryEnvelope, TelemetryRecord};

#[derive(Clone, Debug)]
pub struct TelemetryConfig {
    pub spool_path: PathBuf,
    pub audit_path: PathBuf,
    pub max_records: usize,
    pub max_bytes: usize,
    pub retention: Duration,
}

impl TelemetryConfig {
    pub fn local_only(spool_path: PathBuf, audit_path: PathBuf) -> Self {
        Self {
            spool_path,
            audit_path,
            max_records: 2_048,
            max_bytes: 2 * 1024 * 1024,
            retention: Duration::from_secs(7 * 24 * 60 * 60),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureStatus {
    Captured,
    DroppedFull,
    DroppedBusy,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TelemetryHealth {
    pub queued_records: u64,
    pub queued_bytes: u64,
    pub dropped_records: u64,
    pub write_failures: u64,
    pub export_failures: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RetentionReport {
    pub expired_records: u64,
    pub deleted_session_records: u64,
    pub malformed_records: u64,
    pub failures: u64,
}

#[derive(Debug)]
struct BufferedRecord {
    envelope: TelemetryEnvelope,
    encoded_len: usize,
}

#[derive(Debug, Default)]
struct Spool {
    records: VecDeque<BufferedRecord>,
    bytes: usize,
}

pub struct TelemetryRecorder {
    config: TelemetryConfig,
    spool: Mutex<Spool>,
    sequence: AtomicU64,
    dropped: AtomicU64,
    write_failures: AtomicU64,
    export_failures: AtomicU64,
    exporter: Option<Arc<dyn OtlpExporter>>,
}

impl std::fmt::Debug for TelemetryRecorder {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TelemetryRecorder")
            .field("config", &self.config)
            .field("export_enabled", &self.exporter.is_some())
            .field("health", &self.health())
            .finish_non_exhaustive()
    }
}

impl TelemetryRecorder {
    /// Creates a strictly local recorder. No network-capable object exists on this path.
    pub fn local_only(config: TelemetryConfig) -> Self {
        Self {
            config,
            spool: Mutex::new(Spool::default()),
            sequence: AtomicU64::new(1),
            dropped: AtomicU64::new(0),
            write_failures: AtomicU64::new(0),
            export_failures: AtomicU64::new(0),
            exporter: None,
        }
    }

    /// Explicit opt-in constructor. Merely installing an exporter never sends data.
    pub fn with_otlp_exporter(config: TelemetryConfig, exporter: Arc<dyn OtlpExporter>) -> Self {
        let mut recorder = Self::local_only(config);
        recorder.exporter = Some(exporter);
        recorder
    }

    /// Non-blocking hot-path capture. Lock contention and capacity pressure are counted drops;
    /// neither telemetry I/O nor exporter code runs on the operation thread.
    pub fn capture(&self, record: TelemetryRecord) -> CaptureStatus {
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        let envelope = TelemetryEnvelope {
            schema_version: TelemetryEnvelope::SCHEMA_VERSION,
            sequence,
            observed_at_unix_ms: now_ms(),
            record,
        };
        let encoded_len = serde_json::to_vec(&envelope)
            .map(|bytes| bytes.len() + 1)
            .unwrap_or(usize::MAX);
        let mut spool = match self.spool.try_lock() {
            Ok(spool) => spool,
            Err(TryLockError::WouldBlock) | Err(TryLockError::Poisoned(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                return CaptureStatus::DroppedBusy;
            }
        };
        if encoded_len == usize::MAX
            || spool.records.len() >= self.config.max_records
            || encoded_len > self.config.max_bytes.saturating_sub(spool.bytes)
        {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return CaptureStatus::DroppedFull;
        }
        spool.bytes += encoded_len;
        spool.records.push_back(BufferedRecord {
            envelope,
            encoded_len,
        });
        CaptureStatus::Captured
    }

    pub fn health(&self) -> TelemetryHealth {
        let (queued_records, queued_bytes) = self
            .spool
            .try_lock()
            .map(|spool| (spool.records.len() as u64, spool.bytes as u64))
            .unwrap_or((0, 0));
        TelemetryHealth {
            queued_records,
            queued_bytes,
            dropped_records: self.dropped.load(Ordering::Relaxed),
            write_failures: self.write_failures.load(Ordering::Relaxed),
            export_failures: self.export_failures.load(Ordering::Relaxed),
        }
    }

    /// Explicit maintenance-path local flush. Audit records use a separate append-only file.
    pub fn flush_local(&self) -> usize {
        let mut spool = match self.spool.lock() {
            Ok(spool) => spool,
            Err(poisoned) => poisoned.into_inner(),
        };
        let mut flushed = 0;
        while let Some(front) = spool.records.front() {
            let path = match &front.envelope.record {
                TelemetryRecord::Audit(_) => &self.config.audit_path,
                _ => &self.config.spool_path,
            };
            if append_envelope(path, &front.envelope).is_err() {
                self.write_failures.fetch_add(1, Ordering::Relaxed);
                break;
            }
            let removed = spool.records.pop_front().expect("front record exists");
            spool.bytes = spool.bytes.saturating_sub(removed.encoded_len);
            flushed += 1;
        }
        flushed
    }

    /// Sends a snapshot only when explicitly configured. Export failure leaves the local queue
    /// intact and is reported through health counters rather than the caller's operation result.
    pub fn export_pending(&self) -> ExportStatus {
        let Some(exporter) = &self.exporter else {
            return ExportStatus::Disabled;
        };
        let batch = match self.spool.lock() {
            Ok(spool) => spool
                .records
                .iter()
                .map(|item| item.envelope.clone())
                .collect::<Vec<_>>(),
            Err(poisoned) => poisoned
                .into_inner()
                .records
                .iter()
                .map(|item| item.envelope.clone())
                .collect::<Vec<_>>(),
        };
        if batch.is_empty() {
            return ExportStatus::Empty;
        }
        match exporter.export(&batch) {
            Ok(()) => ExportStatus::Exported {
                records: batch.len(),
            },
            Err(error) => {
                self.export_failures.fetch_add(1, Ordering::Relaxed);
                ExportStatus::Failed(error)
            }
        }
    }

    pub fn enforce_retention(&self, now: SystemTime) -> RetentionReport {
        let cutoff = now
            .checked_sub(self.config.retention)
            .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
            .map(|value| value.as_millis().min(u64::MAX as u128) as u64)
            .unwrap_or(0);
        let mut report = RetentionReport::default();
        for path in [&self.config.spool_path, &self.config.audit_path] {
            match rewrite_filtered(path, |envelope| envelope.observed_at_unix_ms >= cutoff) {
                Ok(file_report) => {
                    report.expired_records += file_report.removed;
                    report.malformed_records += file_report.malformed;
                }
                Err(()) => report.failures += 1,
            }
        }
        if report.failures > 0 {
            self.write_failures
                .fetch_add(report.failures, Ordering::Relaxed);
        }
        report
    }

    pub fn delete_session(&self, session_id: &PrivateId) -> RetentionReport {
        let mut report = RetentionReport::default();
        {
            let mut spool = match self.spool.lock() {
                Ok(spool) => spool,
                Err(poisoned) => poisoned.into_inner(),
            };
            let before = spool.records.len();
            spool
                .records
                .retain(|item| record_session(&item.envelope.record) != Some(session_id));
            spool.bytes = spool.records.iter().map(|item| item.encoded_len).sum();
            report.deleted_session_records += (before - spool.records.len()) as u64;
        }
        for path in [&self.config.spool_path, &self.config.audit_path] {
            match rewrite_filtered(path, |envelope| {
                record_session(&envelope.record) != Some(session_id)
            }) {
                Ok(file_report) => {
                    report.deleted_session_records += file_report.removed;
                    report.malformed_records += file_report.malformed;
                }
                Err(()) => report.failures += 1,
            }
        }
        if report.failures > 0 {
            self.write_failures
                .fetch_add(report.failures, Ordering::Relaxed);
        }
        report
    }
}

fn record_session(record: &TelemetryRecord) -> Option<&PrivateId> {
    match record {
        TelemetryRecord::Trace(event) => Some(&event.ids.session_id),
        TelemetryRecord::Metric(event) => event.ids.as_ref().map(|ids| &ids.session_id),
        TelemetryRecord::Audit(event) => Some(&event.ids.session_id),
    }
}

fn append_envelope(path: &Path, envelope: &TelemetryEnvelope) -> Result<(), ()> {
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
struct FileFilterReport {
    removed: u64,
    malformed: u64,
}

fn rewrite_filtered(
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

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}
