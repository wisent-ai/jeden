//! Every way attaching something can be refused, each said in terms the
//! operator can act on.
//!
//! Split out of `tui/editor/attachments.rs`, which had grown past the module
//! line cap.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachmentError {
    Empty,
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
            Self::Empty => write!(formatter, "Attachment is empty"),
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

pub(super) fn format_bytes(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}
