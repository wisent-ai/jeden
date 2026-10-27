use super::{ExecutionGrant, GrantError};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

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
        // An artifact root not made yet is judged by the directory it will be
        // made in.
        let canonical = match self.root.canonicalize() {
            Ok(canonical) => canonical,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let parent = self.root.parent().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::PermissionDenied, "artifact root has no parent")
                })?;
                let name = self.root.file_name().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::PermissionDenied, "artifact root has no name")
                })?;
                parent.canonicalize()?.join(name)
            }
            Err(error) => return Err(error),
        };
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

    /// The most bytes one artifact may hold: the grant's file limits, or no
    /// limit for an operation run under none.
    fn maximum_bytes(&self) -> Option<u64> {
        self.grant.as_ref().map(|grant| {
            grant
                .filesystem
                .max_file_bytes
                .min(grant.resource_limits.file_bytes)
        })
    }

    fn create_output(&self, stream: &str) -> io::Result<(PathBuf, File)> {
        self.authorize_root()?;
        fs::create_dir_all(&self.root)?;
        let name = format!("operation-{}-{stream}.log", uuid::Uuid::new_v4());
        let path = self.root.join(name);
        let file = OpenOptions::new().write(true).create_new(true).open(&path)?;
        Ok((path, file))
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
            total_bytes: crate::tally::ORIGIN as u64,
            truncated: false,
            artifact: None,
            sha256: hex::encode(Sha256::digest([])),
        }
    }
}

/// A stream's output: whole when no limits are stated, and with stated limits
/// its head and tail inline and the whole of it in an artifact.
pub struct BoundedOutput {
    stream: &'static str,
    limits: Option<OutputLimits>,
    artifacts: ArtifactSink,
    head: Vec<u8>,
    tail: Vec<u8>,
    total_bytes: u64,
    digest: Sha256,
    spill: Option<(PathBuf, File)>,
}

impl BoundedOutput {
    pub fn new(
        stream: &'static str,
        limits: Option<OutputLimits>,
        artifacts: ArtifactSink,
    ) -> Self {
        Self {
            stream,
            limits,
            artifacts,
            head: Vec::new(),
            tail: Vec::new(),
            total_bytes: crate::tally::ORIGIN as u64,
            digest: Sha256::new(),
            spill: None,
        }
    }

    pub fn write_chunk(&mut self, bytes: &[u8]) -> io::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        let grown = self.total_bytes.saturating_add(bytes.len() as u64);
        if self.artifacts.maximum_bytes().is_some_and(|maximum| grown > maximum) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "artifact output exceeds execution grant file limit",
            ));
        }
        self.total_bytes = grown;
        self.digest.update(bytes);
        let Some(limits) = self.limits else {
            self.head.extend_from_slice(bytes);
            return Ok(());
        };
        if self.spill.is_none() && self.total_bytes > limits.inline_bytes() as u64 {
            let (path, mut file) = self.artifacts.create_output(self.stream)?;
            file.write_all(&self.head)?;
            file.write_all(&self.tail)?;
            self.spill = Some((path, file));
        }
        // The bytes held inline were copied when the spill started; each
        // chunk after that is appended.
        if let Some((_, file)) = self.spill.as_mut() {
            file.write_all(bytes)?;
        }

        let head_remaining = limits.head_bytes.saturating_sub(self.head.len());
        let head_take = head_remaining.min(bytes.len());
        self.head.extend_from_slice(&bytes[..head_take]);

        // The tail keeps the last `tail_bytes` of what the head did not take.
        self.tail.extend_from_slice(&bytes[head_take..]);
        let excess = self.tail.len().saturating_sub(limits.tail_bytes);
        self.tail.drain(..excess);
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
