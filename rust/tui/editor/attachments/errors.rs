//! Every way attaching something can be refused, each said in terms the
//! operator can act on.
//!
//! Split out of `tui/editor/attachments.rs`, which had grown past the module
//! line cap.

pub const MAX_ATTACHMENTS: usize = 10;
pub const MAX_ATTACHMENT_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_TOTAL_ATTACHMENT_BYTES: usize = 20 * 1024 * 1024;

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

pub(super) fn format_bytes(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}
