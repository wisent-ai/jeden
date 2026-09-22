//! Deciding what an attached file actually is by reading its bytes rather
//! than trusting its name.
//!
//! Split out of `tui/editor/attachments.rs`, which had grown past the module
//! line cap.

use super::{AttachmentError, AttachmentKind};

pub(super) fn sniff_kind(bytes: &[u8]) -> Result<AttachmentKind, AttachmentError> {
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
