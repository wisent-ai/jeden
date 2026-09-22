//! The tray of things attached to the next message: what is in it, what may
//! be added, and what it weighs.

use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

mod errors;
mod sniff;

pub use errors::{
    AttachmentError, MAX_ATTACHMENTS, MAX_ATTACHMENT_BYTES, MAX_TOTAL_ATTACHMENT_BYTES,
};
use errors::format_bytes;
use sniff::sniff_kind;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AttachmentId(pub u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachmentSource {
    Clipboard,
    File { basename: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachmentKind {
    Text {
        mime: String,
    },
    Image {
        mime: String,
        width: u32,
        height: u32,
    },
    Binary {
        mime: String,
    },
}

#[derive(Debug, Clone)]
pub struct Attachment {
    pub id: AttachmentId,
    pub name: String,
    pub source: AttachmentSource,
    pub kind: AttachmentKind,
    bytes: Arc<[u8]>,
}

impl Attachment {
    /// Clone the shared byte handle without copying the attachment payload.
    pub fn bytes(&self) -> Arc<[u8]> {
        Arc::clone(&self.bytes)
    }
    pub fn fallback_label(&self) -> String {
        match &self.kind {
            AttachmentKind::Image { width, height, .. } => {
                format!(
                    "[image {}x{}, {}, {}]",
                    width,
                    height,
                    format_bytes(self.bytes.len()),
                    self.name
                )
            }
            AttachmentKind::Text { mime } | AttachmentKind::Binary { mime } => {
                format!(
                    "[attachment {}, {}, {}]",
                    mime,
                    format_bytes(self.bytes.len()),
                    self.name
                )
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardContent {
    Text(String),
    Bytes { name: String, bytes: Arc<[u8]> },
}

impl From<String> for ClipboardContent {
    fn from(text: String) -> Self {
        Self::Text(text)
    }
}

impl From<(String, Vec<u8>)> for ClipboardContent {
    fn from((name, bytes): (String, Vec<u8>)) -> Self {
        Self::Bytes {
            name,
            bytes: Arc::from(bytes),
        }
    }
}


#[derive(Debug, Default, Clone)]
pub struct AttachmentTray {
    items: Vec<Attachment>,
    total_bytes: usize,
    next_id: u64,
}

impl AttachmentTray {
    pub fn items(&self) -> &[Attachment] {
        &self.items
    }

    /// Snapshot this tray for exactly one submitted turn without copying bytes.
    pub fn take_all(&mut self) -> Vec<Attachment> {
        self.total_bytes = 0;
        std::mem::take(&mut self.items)
    }
    pub fn add_file(&mut self, cwd: &Path, input: &str) -> Result<AttachmentId, AttachmentError> {
        if input.trim().is_empty() {
            return Err(AttachmentError::Path(
                "usage: /attach <relative-path>".into(),
            ));
        }
        let path =
            crate::tool_runtime::shared::jail_path(cwd, input).map_err(AttachmentError::Path)?;
        let basename = path
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .ok_or_else(|| AttachmentError::Path("path has no UTF-8 basename".into()))?
            .to_string();

        // Reject a full tray before opening the file, then read from one handle
        // with limit+1 so a growing or metadata-racing file cannot bypass limits.
        self.check_limits(1)?;
        let file = File::open(&path).map_err(|error| AttachmentError::Io(error.to_string()))?;
        let mut bytes = Vec::new();
        file.take((MAX_ATTACHMENT_BYTES as u64).saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|error| AttachmentError::Io(error.to_string()))?;
        if bytes.len() > MAX_ATTACHMENT_BYTES {
            return Err(AttachmentError::ItemLimit {
                limit_bytes: MAX_ATTACHMENT_BYTES,
                actual_bytes: bytes.len(),
            });
        }
        self.add_bytes(
            basename.clone(),
            AttachmentSource::File { basename },
            Arc::from(bytes),
        )
    }

    pub fn add_clipboard(
        &mut self,
        content: ClipboardContent,
    ) -> Result<Option<AttachmentId>, AttachmentError> {
        match content {
            ClipboardContent::Text(_) => Ok(None),
            ClipboardContent::Bytes { name, bytes } => self
                .add_bytes(name, AttachmentSource::Clipboard, bytes)
                .map(Some),
        }
    }

    fn add_bytes(
        &mut self,
        name: String,
        source: AttachmentSource,
        bytes: Arc<[u8]>,
    ) -> Result<AttachmentId, AttachmentError> {
        self.check_limits(bytes.len())?;
        let kind = sniff_kind(&bytes)?;
        match &kind {
            AttachmentKind::Text { .. }
                if bytes.len() > crate::model_router::MAX_TEXT_ATTACHMENT_BYTES =>
            {
                return Err(AttachmentError::TextLimit {
                    limit_bytes: crate::model_router::MAX_TEXT_ATTACHMENT_BYTES,
                    actual_bytes: bytes.len(),
                });
            }
            AttachmentKind::Binary { mime } => {
                return Err(AttachmentError::UnsupportedBinary { mime: mime.clone() });
            }
            _ => {}
        }
        let id = AttachmentId(self.next_id);
        self.next_id = self.next_id.wrapping_add(1);
        self.total_bytes += bytes.len();
        self.items.push(Attachment {
            id,
            name,
            source,
            kind,
            bytes,
        });
        Ok(id)
    }

    pub fn remove(&mut self, id: AttachmentId) -> Option<Attachment> {
        let index = self.items.iter().position(|item| item.id == id)?;
        let item = self.items.remove(index);
        self.total_bytes = self.total_bytes.saturating_sub(item.bytes.len());
        Some(item)
    }

    fn check_limits(&self, bytes: usize) -> Result<(), AttachmentError> {
        if self.items.len() >= MAX_ATTACHMENTS {
            return Err(AttachmentError::CountLimit {
                limit: MAX_ATTACHMENTS,
            });
        }
        if bytes == 0 {
            return Err(AttachmentError::Empty);
        }
        if bytes > MAX_ATTACHMENT_BYTES {
            return Err(AttachmentError::ItemLimit {
                limit_bytes: MAX_ATTACHMENT_BYTES,
                actual_bytes: bytes,
            });
        }
        let total = self.total_bytes.saturating_add(bytes);
        if total > MAX_TOTAL_ATTACHMENT_BYTES {
            return Err(AttachmentError::TotalLimit {
                limit_bytes: MAX_TOTAL_ATTACHMENT_BYTES,
                actual_bytes: total,
            });
        }
        Ok(())
    }
}
