use super::{ExecutionGrant, GrantError};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static ARTIFACT_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug)]
pub struct OutputLimits {
    pub head_bytes: usize,
    pub tail_bytes: usize,
}

impl OutputLimits {
    pub fn inline_bytes(self) -> usize {
        self.head_bytes.saturating_add(self.tail_bytes)
    }
}

impl Default for OutputLimits {
    fn default() -> Self {
        Self {
            head_bytes: 32 * 1024,
            tail_bytes: 32 * 1024,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ArtifactSink {
    root: PathBuf,
    grant: Option<ExecutionGrant>,
}

impl ArtifactSink {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            grant: None,
        }
    }

    pub(crate) fn with_grant(mut self, grant: ExecutionGrant) -> Self {
        self.grant = Some(grant);
        self
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn authorize_root(&self) -> io::Result<()> {
        let Some(grant) = &self.grant else {
            return Ok(());
        };
        if grant.is_expired() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                GrantError::Expired,
            ));
        }
        let canonical = self.root.canonicalize().or_else(|_| {
            let parent = self.root.parent().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "artifact root has no parent",
                )
            })?;
            Ok::<PathBuf, io::Error>(parent.canonicalize()?.join(
                self.root.file_name().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::PermissionDenied, "artifact root has no name")
                })?,
            ))
        })?;
        if !grant
            .filesystem
            .write_roots
            .iter()
            .any(|root| canonical.starts_with(root))
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                GrantError::FilesystemDenied(format!(
                    "artifact root {} is outside grant",
                    canonical.display()
                )),
            ));
        }
        Ok(())
    }

    fn maximum_bytes(&self) -> u64 {
        self.grant.as_ref().map_or(u64::MAX, |grant| {
            grant
                .filesystem
                .max_file_bytes
                .min(grant.resource_limits.file_bytes)
        })
    }

    fn create_output(&self, stream: &str) -> io::Result<(PathBuf, File)> {
        self.authorize_root()?;
        fs::create_dir_all(&self.root)?;
        loop {
            let sequence = ARTIFACT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let name = format!("operation-{}-{sequence}-{stream}.log", std::process::id());
            let path = self.root.join(name);
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => return Ok((path, file)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
    }
}

#[derive(Debug)]
pub struct OutputCapture {
    pub text: String,
    pub head: String,
    pub tail: String,
    pub total_bytes: u64,
    pub truncated: bool,
    pub artifact: Option<PathBuf>,
    pub sha256: String,
}

impl OutputCapture {
    pub(crate) fn uncaptured() -> Self {
        Self {
            text: String::new(),
            head: String::new(),
            tail: String::new(),
            total_bytes: 0,
            truncated: false,
            artifact: None,
            sha256: hex::encode(Sha256::digest([])),
        }
    }
}

pub struct BoundedOutput {
    stream: &'static str,
    limits: OutputLimits,
    artifacts: ArtifactSink,
    head: Vec<u8>,
    tail: Vec<u8>,
    total_bytes: u64,
    digest: Sha256,
    spill: Option<(PathBuf, File)>,
}

impl BoundedOutput {
    pub fn new(stream: &'static str, limits: OutputLimits, artifacts: ArtifactSink) -> Self {
        Self {
            stream,
            limits,
            artifacts,
            head: Vec::with_capacity(limits.head_bytes),
            tail: Vec::with_capacity(limits.tail_bytes),
            total_bytes: 0,
            digest: Sha256::new(),
            spill: None,
        }
    }

    pub fn write_chunk(&mut self, bytes: &[u8]) -> io::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        if self.total_bytes.saturating_add(bytes.len() as u64) > self.artifacts.maximum_bytes() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "artifact output exceeds execution grant file limit",
            ));
        }
        let previous_total = self.total_bytes;
        self.total_bytes = self.total_bytes.saturating_add(bytes.len() as u64);
        self.digest.update(bytes);

        if self.spill.is_none() && self.total_bytes > self.limits.inline_bytes() as u64 {
            let (path, mut file) = self.artifacts.create_output(self.stream)?;
            file.write_all(&self.head)?;
            file.write_all(&self.tail)?;
            self.spill = Some((path, file));
        }
        if let Some((_, file)) = self.spill.as_mut() {
            if previous_total > 0 && previous_total <= self.limits.inline_bytes() as u64 {
                // Existing bounded bytes were copied when spill started; only append this chunk.
            }
            file.write_all(bytes)?;
        }

        let head_remaining = self.limits.head_bytes.saturating_sub(self.head.len());
        let head_take = head_remaining.min(bytes.len());
        self.head.extend_from_slice(&bytes[..head_take]);

        if self.limits.tail_bytes > 0 {
            let tail_input = &bytes[head_take..];
            if tail_input.len() >= self.limits.tail_bytes {
                self.tail.clear();
                self.tail
                    .extend_from_slice(&tail_input[tail_input.len() - self.limits.tail_bytes..]);
            } else if !tail_input.is_empty() {
                let overflow = self
                    .tail
                    .len()
                    .saturating_add(tail_input.len())
                    .saturating_sub(self.limits.tail_bytes);
                if overflow > 0 {
                    self.tail.copy_within(overflow.., 0);
                    self.tail.truncate(self.tail.len() - overflow);
                }
                self.tail.extend_from_slice(tail_input);
            }
        }
        Ok(())
    }

    pub fn finish(mut self) -> io::Result<OutputCapture> {
        let artifact = if let Some((path, mut file)) = self.spill.take() {
            file.flush()?;
            file.sync_all()?;
            Some(path)
        } else {
            None
        };
        let head = String::from_utf8_lossy(&self.head).into_owned();
        let tail = String::from_utf8_lossy(&self.tail).into_owned();
        let truncated = artifact.is_some();
        let text = if truncated {
            format!(
                "{head}\n[... {} bytes omitted; full output in artifact ...]\n{tail}",
                self.total_bytes
                    .saturating_sub(self.head.len().saturating_add(self.tail.len()) as u64)
            )
        } else {
            format!("{head}{tail}")
        };
        Ok(OutputCapture {
            text,
            head,
            tail,
            total_bytes: self.total_bytes,
            truncated,
            artifact,
            sha256: hex::encode(self.digest.finalize()),
        })
    }
}
