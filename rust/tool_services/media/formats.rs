use super::super::types::{ServiceError, ServiceResult};
use super::router::check_job_id;
use super::CapabilitySubmission;
use serde_json::Value;

pub(super) fn optional_string(input: &Value, key: &str) -> Option<String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

pub(super) fn dimensions(size: Option<&str>) -> ServiceResult<(Option<u32>, Option<u32>)> {
    let Some(size) = size else {
        return Ok((None, None));
    };
    let (width, height) = size
        .split_once('x')
        .ok_or_else(|| ServiceError::InvalidInput("size must be WIDTHxHEIGHT".into()))?;
    let width = width
        .parse()
        .map_err(|_| ServiceError::InvalidInput("invalid image width".into()))?;
    let height = height
        .parse()
        .map_err(|_| ServiceError::InvalidInput("invalid image height".into()))?;
    Ok((Some(width), Some(height)))
}

pub(super) fn aspect_ratio(size: Option<&str>) -> ServiceResult<Option<String>> {
    let (width, height) = dimensions(size)?;
    Ok(width
        .zip(height)
        .map(|(width, height)| format!("{width}:{height}")))
}

pub(super) fn image_extension(mime_type: &str) -> ServiceResult<&'static str> {
    match mime_type.trim().to_ascii_lowercase().as_str() {
        "image/png" => Ok("png"),
        "image/jpeg" | "image/jpg" => Ok("jpg"),
        "image/webp" => Ok("webp"),
        "image/gif" => Ok("gif"),
        other => Err(ServiceError::Protocol {
            service: "media-router",
            detail: format!("unsupported image Content-Type {other}"),
        }),
    }
}

pub(super) fn validate_submission(submission: CapabilitySubmission) -> ServiceResult<String> {
    if !submission.success {
        return Err(ServiceError::Backend {
            service: "media-router",
            detail: submission
                .error
                .unwrap_or_else(|| "media submission reported failure".into()),
        });
    }
    check_job_id(&submission.job_id)?;
    if submission.status.trim().is_empty() {
        return Err(ServiceError::Protocol {
            service: "media-router",
            detail: "media submission lacks status".into(),
        });
    }
    Ok(submission.job_id)
}

pub(super) fn image_metadata(bytes: &[u8]) -> ServiceResult<(&'static str, u32, u32)> {
    if bytes.len() >= 24 && &bytes[..8] == b"\x89PNG\r\n\x1a\n" {
        return Ok((
            "png",
            u32::from_be_bytes(bytes[16..20].try_into().unwrap()),
            u32::from_be_bytes(bytes[20..24].try_into().unwrap()),
        ));
    }
    if bytes.len() >= 10 && matches!(&bytes[..6], b"GIF87a" | b"GIF89a") {
        return Ok((
            "gif",
            u16::from_le_bytes(bytes[6..8].try_into().unwrap()) as u32,
            u16::from_le_bytes(bytes[8..10].try_into().unwrap()) as u32,
        ));
    }
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Ok(("webp", 0, 0));
    }
    if bytes.starts_with(&[0xff, 0xd8]) {
        let mut i = 2;
        while i + 9 < bytes.len() {
            if bytes[i] != 0xff {
                i += 1;
                continue;
            }
            let marker = bytes[i + 1];
            if matches!(
                marker,
                0xc0 | 0xc1
                    | 0xc2
                    | 0xc3
                    | 0xc5
                    | 0xc6
                    | 0xc7
                    | 0xc9
                    | 0xca
                    | 0xcb
                    | 0xcd
                    | 0xce
                    | 0xcf
            ) {
                return Ok((
                    "jpeg",
                    u16::from_be_bytes([bytes[i + 7], bytes[i + 8]]) as u32,
                    u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]) as u32,
                ));
            }
            if i + 4 > bytes.len() {
                break;
            }
            let len = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
            if len < 2 {
                break;
            }
            i += 2 + len
        }
        return Err(ServiceError::Protocol {
            service: "image",
            detail: "malformed JPEG".into(),
        });
    }
    Err(ServiceError::InvalidInput(
        "unsupported image format".into(),
    ))
}
