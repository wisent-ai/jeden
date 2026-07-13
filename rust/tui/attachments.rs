use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

pub const MAX_ATTACHMENTS: usize = 10;
pub const MAX_ATTACHMENT_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_TOTAL_ATTACHMENT_BYTES: usize = 20 * 1024 * 1024;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachmentError {
    CountLimit {
        limit: usize,
    },
    ItemLimit {
        limit_bytes: usize,
        actual_bytes: usize,
    },
    TotalLimit {
        limit_bytes: usize,
        actual_bytes: usize,
    },
    Empty,
    TextLimit {
        limit_bytes: usize,
        actual_bytes: usize,
    },
    UnsupportedBinary {
        mime: String,
    },
    Path(String),
    Io(String),
    InvalidImage(String),
}

impl std::fmt::Display for AttachmentError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CountLimit { limit } => write!(formatter, "Attachment limit reached ({limit})"),
            Self::ItemLimit {
                limit_bytes,
                actual_bytes,
            } => write!(
                formatter,
                "Attachment is {actual_bytes} bytes; per-item limit is {limit_bytes}"
            ),
            Self::TotalLimit {
                limit_bytes,
                actual_bytes,
            } => write!(
                formatter,
                "Attachments total {actual_bytes} bytes; total limit is {limit_bytes}"
            ),
            Self::Empty => write!(formatter, "Attachment is empty"),
            Self::TextLimit {
                limit_bytes,
                actual_bytes,
            } => write!(
                formatter,
                "Text attachment is {actual_bytes} bytes; text limit is {limit_bytes}"
            ),
            Self::UnsupportedBinary { mime } => {
                write!(formatter, "Unsupported binary attachment type `{mime}`")
            }
            Self::Path(error) => write!(formatter, "Attachment path rejected: {error}"),
            Self::Io(error) => write!(formatter, "Attachment read failed: {error}"),
            Self::InvalidImage(error) => write!(formatter, "Invalid image: {error}"),
        }
    }
}

impl std::error::Error for AttachmentError {}

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

fn sniff_kind(bytes: &[u8]) -> Result<AttachmentKind, AttachmentError> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        if bytes.len() < 24 {
            return Err(AttachmentError::InvalidImage("truncated PNG header".into()));
        }
        let width = u32::from_be_bytes(bytes[16..20].try_into().expect("fixed PNG width slice"));
        let height = u32::from_be_bytes(bytes[20..24].try_into().expect("fixed PNG height slice"));
        if width == 0 || height == 0 {
            return Err(AttachmentError::InvalidImage("zero PNG dimension".into()));
        }
        return Ok(AttachmentKind::Image {
            mime: "image/png".into(),
            width,
            height,
        });
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        if bytes.len() < 10 {
            return Err(AttachmentError::InvalidImage("truncated GIF header".into()));
        }
        let width = u16::from_le_bytes([bytes[6], bytes[7]]) as u32;
        let height = u16::from_le_bytes([bytes[8], bytes[9]]) as u32;
        if width == 0 || height == 0 {
            return Err(AttachmentError::InvalidImage("zero GIF dimension".into()));
        }
        return Ok(AttachmentKind::Image {
            mime: "image/gif".into(),
            width,
            height,
        });
    }
    if bytes.starts_with(&[0xff, 0xd8]) {
        let (width, height) = jpeg_dimensions(bytes)?;
        return Ok(AttachmentKind::Image {
            mime: "image/jpeg".into(),
            width,
            height,
        });
    }
    if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        let (width, height) = webp_dimensions(bytes)?;
        return Ok(AttachmentKind::Image {
            mime: "image/webp".into(),
            width,
            height,
        });
    }
    if std::str::from_utf8(bytes).is_ok() {
        return Ok(AttachmentKind::Text {
            mime: "text/plain; charset=utf-8".into(),
        });
    }
    Ok(AttachmentKind::Binary {
        mime: "application/octet-stream".into(),
    })
}

fn webp_dimensions(bytes: &[u8]) -> Result<(u32, u32), AttachmentError> {
    if bytes.len() < 20 {
        return Err(AttachmentError::InvalidImage(
            "truncated WebP header".into(),
        ));
    }
    let chunk_len =
        u32::from_le_bytes(bytes[16..20].try_into().expect("WebP chunk length")) as usize;
    if chunk_len > bytes.len().saturating_sub(20) {
        return Err(AttachmentError::InvalidImage("truncated WebP chunk".into()));
    }
    let data = &bytes[20..20 + chunk_len];
    match &bytes[12..16] {
        b"VP8 " => {
            if data.len() < 10 || data.get(3..6) != Some(&[0x9d, 0x01, 0x2a]) {
                return Err(AttachmentError::InvalidImage(
                    "invalid VP8 frame header".into(),
                ));
            }
            let width = u16::from_le_bytes([data[6], data[7]]) & 0x3fff;
            let height = u16::from_le_bytes([data[8], data[9]]) & 0x3fff;
            if width == 0 || height == 0 {
                return Err(AttachmentError::InvalidImage("zero WebP dimension".into()));
            }
            Ok((u32::from(width), u32::from(height)))
        }
        b"VP8L" => {
            if data.len() < 5 || data[0] != 0x2f {
                return Err(AttachmentError::InvalidImage(
                    "invalid VP8L frame header".into(),
                ));
            }
            let bits = u32::from_le_bytes([data[1], data[2], data[3], data[4]]);
            Ok(((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1))
        }
        b"VP8X" => {
            if data.len() < 10 {
                return Err(AttachmentError::InvalidImage(
                    "truncated VP8X header".into(),
                ));
            }
            let width =
                1 + u32::from(data[4]) + (u32::from(data[5]) << 8) + (u32::from(data[6]) << 16);
            let height =
                1 + u32::from(data[7]) + (u32::from(data[8]) << 8) + (u32::from(data[9]) << 16);
            Ok((width, height))
        }
        _ => Err(AttachmentError::InvalidImage(
            "unsupported WebP chunk".into(),
        )),
    }
}

fn jpeg_dimensions(bytes: &[u8]) -> Result<(u32, u32), AttachmentError> {
    let mut offset = 2usize;
    while offset + 4 <= bytes.len() {
        if bytes[offset] != 0xff {
            offset += 1;
            continue;
        }
        let marker = bytes[offset + 1];
        offset += 2;
        if matches!(marker, 0xd8 | 0xd9) {
            continue;
        }
        let length = u16::from_be_bytes([bytes[offset], bytes[offset + 1]]) as usize;
        if length < 2 || offset + length > bytes.len() {
            break;
        }
        if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) && length >= 7 {
            let height = u16::from_be_bytes([bytes[offset + 3], bytes[offset + 4]]) as u32;
            let width = u16::from_be_bytes([bytes[offset + 5], bytes[offset + 6]]) as u32;
            if width > 0 && height > 0 {
                return Ok((width, height));
            }
        }
        offset += length;
    }
    Err(AttachmentError::InvalidImage(
        "JPEG dimensions not found".into(),
    ))
}

fn format_bytes(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}
