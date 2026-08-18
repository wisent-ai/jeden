use super::config;
use super::types::{
    bounded_json, check_operation, nonempty, write_media_artifact, HealthDescriptor, ServiceError,
    ServiceResult,
};
use crate::tool_runtime::runtime_ops::OperationContext;
use base64::Engine;
use reqwest::{blocking::Client, Url};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
const MAX_INPUT_IMAGE: usize = 20 * 1024 * 1024;
pub(crate) const TOOLS: &[(&str, &str)] = &[
    (
        "image_inspect",
        "Inspect image format, dimensions, size, and digest",
    ),
    (
        "image_generate",
        "Generate an image through the authenticated Stado media router and preserve it as an artifact",
    ),
    (
        "image_edit",
        "Edit an image through the authenticated Stado media router and preserve it as an artifact",
    ),
    (
        "tts",
        "Synthesize speech through the authenticated Stado media router and preserve it as an artifact",
    ),
];

#[derive(Serialize)]
struct ImageGenerateRequest {
    prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    style: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    negative_prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    height: Option<u32>,
}

#[derive(Deserialize)]
struct ImageGenerateResponse {
    success: bool,
    job_id: String,
    image_base64: Option<String>,
    mime_type: Option<String>,
    error: Option<String>,
}

#[derive(Serialize)]
struct EncodedMediaSample {
    data_base64: String,
    content_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    filename: Option<String>,
}

#[derive(Serialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
enum CapabilityRequest {
    KieImage {
        provider: String,
        action: String,
        model: String,
        prompt: String,
        image: EncodedMediaSample,
        #[serde(skip_serializing_if = "Option::is_none")]
        aspect_ratio: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        quality: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        style: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        negative_prompt: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        seed: Option<i64>,
    },
    TextToSpeech {
        provider: String,
        text: String,
        voice_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        format: Option<String>,
    },
}

#[derive(Deserialize)]
struct CapabilitySubmission {
    success: bool,
    job_id: String,
    status: String,
    error: Option<String>,
}

#[derive(Deserialize)]
struct CapabilityStatus {
    job_id: String,
    status: String,
    error: Option<String>,
}

struct MediaRouterClient {
    origin: Url,
    token: String,
    client: Client,
}

impl MediaRouterClient {
    fn configured() -> Result<Self, String> {
        let raw_origin = std::env::var("STADO_MEDIA_ROUTER_URL")
            .map_err(|_| "STADO_MEDIA_ROUTER_URL is required".to_string())?;
        let token = std::env::var("JEDEN_MEDIA_ROUTER_TOKEN")
            .map_err(|_| "JEDEN_MEDIA_ROUTER_TOKEN is required".to_string())?;
        if token.trim().is_empty() || token.trim() != token || token.chars().any(char::is_control) {
            return Err("JEDEN_MEDIA_ROUTER_TOKEN is empty or malformed".into());
        }
        let mut origin = Url::parse(raw_origin.trim())
            .map_err(|error| format!("invalid STADO_MEDIA_ROUTER_URL: {error}"))?;
        let loopback = matches!(origin.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
        let insecure_loopback = std::env::var("STADO_MEDIA_ROUTER_ALLOW_INSECURE_LOOPBACK")
            .ok()
            .is_some_and(|value| value == "1");
        if origin.scheme() != "https"
            && !(origin.scheme() == "http" && loopback && insecure_loopback)
        {
            return Err(
                "STADO_MEDIA_ROUTER_URL must use HTTPS or explicitly enabled loopback HTTP".into(),
            );
        }
        if !origin.username().is_empty()
            || origin.password().is_some()
            || origin.query().is_some()
            || origin.fragment().is_some()
            || !matches!(origin.path(), "" | "/")
        {
            return Err(
                "STADO_MEDIA_ROUTER_URL must be an origin without credentials or path".into(),
            );
        }
        origin.set_path("/");
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(
                "10".parse().expect("valid media-router connect timeout"),
            ))
            .build()
            .map_err(|error| format!("failed to initialize media-router client: {error}"))?;
        Ok(Self {
            origin,
            token,
            client,
        })
    }

    fn endpoint(&self, path: &str) -> ServiceResult<Url> {
        self.origin
            .join(path)
            .map_err(|error| ServiceError::Protocol {
                service: "media-router",
                detail: error.to_string(),
            })
    }

    fn post_json<T: Serialize, R: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        body: &T,
        context: &OperationContext<'_>,
    ) -> ServiceResult<R> {
        check_operation(context)?;
        let response = self
            .client
            .post(self.endpoint(path)?)
            .timeout(timeout(context))
            .bearer_auth(&self.token)
            .json(body)
            .send()
            .map_err(|error| backend_error("media-router", error))?;
        if !response.status().is_success() {
            return Err(response_failure("media-router", response));
        }
        response.json().map_err(|error| ServiceError::Protocol {
            service: "media-router",
            detail: error.to_string(),
        })
    }

    fn status(
        &self,
        job_id: &str,
        context: &OperationContext<'_>,
    ) -> ServiceResult<CapabilityStatus> {
        check_job_id(job_id)?;
        check_operation(context)?;
        let response = self
            .client
            .get(self.endpoint(&format!("media/{job_id}"))?)
            .timeout(timeout(context))
            .bearer_auth(&self.token)
            .send()
            .map_err(|error| backend_error("media-router", error))?;
        if !response.status().is_success() {
            return Err(response_failure("media-router", response));
        }
        response.json().map_err(|error| ServiceError::Protocol {
            service: "media-router",
            detail: error.to_string(),
        })
    }

    fn content(
        &self,
        job_id: &str,
        expected_type: &str,
        context: &OperationContext<'_>,
    ) -> ServiceResult<(Vec<u8>, String)> {
        check_job_id(job_id)?;
        check_operation(context)?;
        let response = self
            .client
            .get(self.endpoint(&format!("media/{job_id}/content"))?)
            .timeout(timeout(context))
            .bearer_auth(&self.token)
            .send()
            .map_err(|error| backend_error("media-router", error))?;
        if !response.status().is_success() {
            return Err(response_failure("media-router", response));
        }
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| ServiceError::Protocol {
                service: "media-router",
                detail: "media content response lacks Content-Type".into(),
            })?
            .to_ascii_lowercase();
        if !content_type.starts_with(expected_type) {
            return Err(ServiceError::Protocol {
                service: "media-router",
                detail: format!("expected {expected_type} content, received {content_type}"),
            });
        }
        if response
            .content_length()
            .is_some_and(|length| length > "33554432".parse().expect("valid media output limit"))
        {
            return Err(ServiceError::OutputLimit {
                limit: "33554432".parse().expect("valid media output limit"),
            });
        }
        let bytes = response
            .bytes()
            .map_err(|error| backend_error("media-router", error))?
            .to_vec();
        if bytes.is_empty() {
            return Err(ServiceError::Protocol {
                service: "media-router",
                detail: "media content response was empty".into(),
            });
        }
        Ok((bytes, content_type))
    }
}

pub(crate) struct MediaService {
    cwd: PathBuf,
    router: Result<MediaRouterClient, String>,
    image_model: Option<String>,
    tts_model: Option<String>,
}

impl MediaService {
    pub(crate) fn discover(cwd: &Path, value: &Value) -> Self {
        Self {
            cwd: cwd.to_path_buf(),
            router: MediaRouterClient::configured(),
            image_model: config::string(
                value,
                &["toolServices", "image", "model"],
                "JEDEN_IMAGE_MODEL",
            ),
            tts_model: config::string(value, &["toolServices", "tts", "model"], "JEDEN_TTS_MODEL"),
        }
    }

    pub(crate) fn health_for(&self, tool: &str) -> HealthDescriptor {
        match tool {
            "image_inspect" => HealthDescriptor::healthy("image", "builtin"),
            "image_generate" | "image_edit" | "tts" => match &self.router {
                Ok(_) => HealthDescriptor::healthy("media", "stado-media-router"),
                Err(detail) => HealthDescriptor::unavailable("media", detail.clone()),
            },
            _ => HealthDescriptor::unavailable("media", "unknown media tool"),
        }
    }

    pub(crate) fn execute(
        &self,
        tool: &str,
        input: &Value,
        context: &OperationContext<'_>,
    ) -> ServiceResult<Value> {
        match tool {
            "image_inspect" => self.inspect(input, context),
            "image_generate" => self.image_generate(input, context),
            "image_edit" => self.image_edit(input, context),
            "tts" => self.tts_request(input, context),
            _ => Err(ServiceError::InvalidInput(format!(
                "unknown media tool {tool}"
            ))),
        }
    }

    fn router(&self) -> ServiceResult<&MediaRouterClient> {
        self.router
            .as_ref()
            .map_err(|detail| ServiceError::Unavailable {
                service: "media-router",
                detail: detail.clone(),
            })
    }

    fn inspect(&self, input: &Value, context: &OperationContext<'_>) -> ServiceResult<Value> {
        check_operation(context)?;
        let path = self.jailed(input)?;
        let bytes = fs::read(&path)?;
        if bytes.len() > MAX_INPUT_IMAGE {
            return Err(ServiceError::OutputLimit {
                limit: MAX_INPUT_IMAGE,
            });
        }
        let (format, width, height) = image_metadata(&bytes)?;
        bounded_json(
            context,
            "image",
            &json!({"ok":true,"path":path.display().to_string(),"format":format,"width":width,"height":height,"bytes":bytes.len(),"sha256":hex::encode(Sha256::digest(&bytes))}),
        )
    }

    fn image_generate(
        &self,
        input: &Value,
        context: &OperationContext<'_>,
    ) -> ServiceResult<Value> {
        let prompt = nonempty(input.get("prompt"), "prompt")?;
        let (width, height) = dimensions(input.get("size").and_then(Value::as_str))?;
        let request = ImageGenerateRequest {
            prompt,
            provider: optional_string(input, "provider").or_else(|| Some("gemini".into())),
            model: optional_string(input, "model").or_else(|| self.image_model.clone()),
            style: optional_string(input, "style"),
            negative_prompt: optional_string(input, "negative_prompt"),
            width,
            height,
        };
        let response: ImageGenerateResponse =
            self.router()?.post_json("image", &request, context)?;
        if !response.success {
            return Err(ServiceError::Backend {
                service: "media-router",
                detail: response
                    .error
                    .unwrap_or_else(|| "image generation reported failure".into()),
            });
        }
        check_job_id(&response.job_id)?;
        let encoded = response
            .image_base64
            .ok_or_else(|| ServiceError::Protocol {
                service: "media-router",
                detail: "image response lacks image_base64".into(),
            })?;
        let mime_type = response.mime_type.ok_or_else(|| ServiceError::Protocol {
            service: "media-router",
            detail: "image response lacks mime_type".into(),
        })?;
        let extension = image_extension(&mime_type)?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|error| ServiceError::Protocol {
                service: "media-router",
                detail: format!("invalid image_base64: {error}"),
            })?;
        if bytes.is_empty() {
            return Err(ServiceError::Protocol {
                service: "media-router",
                detail: "decoded image was empty".into(),
            });
        }
        let mut artifact = write_media_artifact(context, "image", extension, &bytes)?;
        artifact["provider"] = json!("stado-media-router");
        artifact["jobId"] = json!(response.job_id);
        artifact["mimeType"] = json!(mime_type);
        Ok(artifact)
    }

    fn image_edit(&self, input: &Value, context: &OperationContext<'_>) -> ServiceResult<Value> {
        let prompt = nonempty(input.get("prompt"), "prompt")?;
        let model = optional_string(input, "model")
            .or_else(|| self.image_model.clone())
            .ok_or_else(|| {
                ServiceError::InvalidInput("image_edit requires model or JEDEN_IMAGE_MODEL".into())
            })?;
        let path = self.jailed(input)?;
        let bytes = fs::read(&path)?;
        if bytes.len() > MAX_INPUT_IMAGE {
            return Err(ServiceError::OutputLimit {
                limit: MAX_INPUT_IMAGE,
            });
        }
        let (format, _, _) = image_metadata(&bytes)?;
        let content_type = match format {
            "png" => "image/png",
            "jpeg" => "image/jpeg",
            "gif" => "image/gif",
            "webp" => "image/webp",
            _ => {
                return Err(ServiceError::InvalidInput(
                    "unsupported image edit format".into(),
                ))
            }
        };
        let request = CapabilityRequest::KieImage {
            provider: "kie".into(),
            action: "edit".into(),
            model,
            prompt,
            image: EncodedMediaSample {
                data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
                content_type: content_type.into(),
                filename: path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .map(str::to_owned),
            },
            aspect_ratio: aspect_ratio(input.get("size").and_then(Value::as_str))?,
            quality: optional_string(input, "quality"),
            style: optional_string(input, "style"),
            negative_prompt: optional_string(input, "negative_prompt"),
            seed: input.get("seed").and_then(Value::as_i64),
        };
        let submission: CapabilitySubmission =
            self.router()?.post_json("media", &request, context)?;
        let job_id = validate_submission(submission)?;
        self.wait_for_completion(&job_id, context)?;
        let (output, mime_type) = self.router()?.content(&job_id, "image/", context)?;
        let extension = image_extension(&mime_type)?;
        let mut artifact = write_media_artifact(context, "image", extension, &output)?;
        artifact["provider"] = json!("stado-media-router");
        artifact["jobId"] = json!(job_id);
        artifact["mimeType"] = json!(mime_type);
        Ok(artifact)
    }

    fn tts_request(&self, input: &Value, context: &OperationContext<'_>) -> ServiceResult<Value> {
        let text = nonempty(input.get("text"), "text")?;
        if text.len() > "32000".parse().expect("valid speech input limit") {
            return Err(ServiceError::OutputLimit {
                limit: "32000".parse().expect("valid speech input limit"),
            });
        }
        let provider = optional_string(input, "provider").unwrap_or_else(|| "minimax".into());
        if !matches!(provider.as_str(), "elevenlabs" | "minimax") {
            return Err(ServiceError::InvalidInput(
                "tts provider must be elevenlabs or minimax".into(),
            ));
        }
        let voice_id = optional_string(input, "voice_id")
            .or_else(|| optional_string(input, "voice"))
            .unwrap_or_else(|| "male-qn-qingse".into());
        let format = optional_string(input, "format").unwrap_or_else(|| "mp3".into());
        if !matches!(format.as_str(), "mp3" | "wav" | "opus" | "aac" | "flac") {
            return Err(ServiceError::InvalidInput("unsupported TTS format".into()));
        }
        let request = CapabilityRequest::TextToSpeech {
            provider,
            text,
            voice_id,
            model: optional_string(input, "model").or_else(|| self.tts_model.clone()),
            format: Some(format.clone()),
        };
        let submission: CapabilitySubmission =
            self.router()?.post_json("media", &request, context)?;
        let job_id = validate_submission(submission)?;
        let (bytes, mime_type) = self.router()?.content(&job_id, "audio/", context)?;
        let mut artifact = write_media_artifact(context, "tts", &format, &bytes)?;
        artifact["provider"] = json!("stado-media-router");
        artifact["jobId"] = json!(job_id);
        artifact["mimeType"] = json!(mime_type);
        Ok(artifact)
    }

    fn wait_for_completion(
        &self,
        job_id: &str,
        context: &OperationContext<'_>,
    ) -> ServiceResult<()> {
        let local_deadline = Instant::now()
            + Duration::from_secs("120".parse().expect("valid media polling timeout"));
        let deadline = context
            .deadline()
            .unwrap_or(local_deadline)
            .min(local_deadline);
        loop {
            check_operation(context)?;
            if Instant::now() >= deadline {
                return Err(ServiceError::DeadlineExceeded);
            }
            let status = self.router()?.status(job_id, context)?;
            if status.job_id != job_id {
                return Err(ServiceError::Protocol {
                    service: "media-router",
                    detail: "media status returned a mismatched job_id".into(),
                });
            }
            match status.status.as_str() {
                "completed" => return Ok(()),
                "failed" | "cancelled" | "timed_out" => {
                    return Err(ServiceError::Backend {
                        service: "media-router",
                        detail: status
                            .error
                            .unwrap_or_else(|| format!("media job {}", status.status)),
                    })
                }
                _ => std::thread::sleep(Duration::from_millis(
                    "500".parse().expect("valid media poll interval"),
                )),
            }
        }
    }

    fn jailed(&self, input: &Value) -> ServiceResult<PathBuf> {
        let raw = nonempty(input.get("path"), "path")?;
        let path = PathBuf::from(raw);
        let joined = if path.is_absolute() {
            path
        } else {
            self.cwd.join(path)
        };
        let canonical = joined
            .canonicalize()
            .map_err(|error| ServiceError::Io(error.to_string()))?;
        let root = self
            .cwd
            .canonicalize()
            .map_err(|error| ServiceError::Io(error.to_string()))?;
        if !canonical.starts_with(root) {
            return Err(ServiceError::PermissionDenied(
                "image path escapes workspace".into(),
            ));
        }
        Ok(canonical)
    }
}

fn optional_string(input: &Value, key: &str) -> Option<String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn dimensions(size: Option<&str>) -> ServiceResult<(Option<u32>, Option<u32>)> {
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

fn aspect_ratio(size: Option<&str>) -> ServiceResult<Option<String>> {
    let (width, height) = dimensions(size)?;
    Ok(width
        .zip(height)
        .map(|(width, height)| format!("{width}:{height}")))
}

fn image_extension(mime_type: &str) -> ServiceResult<&'static str> {
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

fn validate_submission(submission: CapabilitySubmission) -> ServiceResult<String> {
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

fn check_job_id(job_id: &str) -> ServiceResult<()> {
    if job_id.is_empty()
        || !job_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(ServiceError::Protocol {
            service: "media-router",
            detail: "media-router returned an invalid job_id".into(),
        });
    }
    Ok(())
}

fn response_failure(service: &'static str, response: reqwest::blocking::Response) -> ServiceError {
    let status = response.status();
    let detail = response
        .text()
        .map(|body| {
            body.chars()
                .take("512".parse().expect("valid error detail limit"))
                .collect::<String>()
        })
        .unwrap_or_else(|_| "response body unavailable".into());
    ServiceError::Backend {
        service,
        detail: format!("HTTP {status}: {detail}"),
    }
}

fn backend_error(service: &'static str, error: reqwest::Error) -> ServiceError {
    ServiceError::Backend {
        service,
        detail: error.to_string(),
    }
}

fn timeout(context: &OperationContext<'_>) -> Duration {
    context
        .deadline()
        .and_then(|deadline| deadline.checked_duration_since(Instant::now()))
        .unwrap_or(Duration::from_secs(
            "90".parse().expect("valid media request timeout"),
        ))
        .min(Duration::from_secs(
            "120".parse().expect("valid maximum media request timeout"),
        ))
}
fn image_metadata(bytes: &[u8]) -> ServiceResult<(&'static str, u32, u32)> {
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
