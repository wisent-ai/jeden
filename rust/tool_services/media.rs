use super::config;
use super::types::{
    bounded_json, check_operation, nonempty, write_media_artifact, HealthDescriptor, ServiceError,
    ServiceResult,
};
use crate::tool_runtime::runtime_ops::OperationContext;
use base64::Engine;
use reqwest::blocking::Client;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

const MAX_INPUT_IMAGE: usize = 20 * 1024 * 1024;
pub(crate) const TOOLS: &[(&str, &str)] = &[
    (
        "image_inspect",
        "Inspect image format, dimensions, size, and digest",
    ),
    (
        "image_generate",
        "Generate an image through a configured provider and preserve it as an artifact",
    ),
    (
        "image_edit",
        "Edit an image through a configured provider and preserve it as an artifact",
    ),
    (
        "tts",
        "Synthesize speech through a configured provider and preserve it as an artifact",
    ),
];
#[derive(Clone)]
struct Provider {
    name: String,
    endpoint: String,
    key: String,
    model: String,
}
pub(crate) struct MediaService {
    cwd: PathBuf,
    image: Vec<Provider>,
    tts: Vec<Provider>,
}
impl MediaService {
    pub(crate) fn discover(cwd: &Path, value: &Value) -> Self {
        let mut image = Vec::new();
        let mut tts = Vec::new();
        if let Some(key) = config::string(
            value,
            &["toolServices", "image", "apiKey"],
            "OPENAI_API_KEY",
        ) {
            let base = config::string(
                value,
                &["toolServices", "image", "baseUrl"],
                "OPENAI_BASE_URL",
            )
            .unwrap_or_else(|| "https://api.openai.com/v1".into());
            image.push(Provider {
                name: "openai".into(),
                endpoint: format!("{}/images/generations", base.trim_end_matches('/')),
                key: key.clone(),
                model: config::string(
                    value,
                    &["toolServices", "image", "model"],
                    "JEDEN_IMAGE_MODEL",
                )
                .unwrap_or_else(|| "gpt-image-1".into()),
            });
            tts.push(Provider {
                name: "openai".into(),
                endpoint: format!("{}/audio/speech", base.trim_end_matches('/')),
                key,
                model: config::string(value, &["toolServices", "tts", "model"], "JEDEN_TTS_MODEL")
                    .unwrap_or_else(|| "gpt-4o-mini-tts".into()),
            });
        }
        Self {
            cwd: cwd.to_path_buf(),
            image,
            tts,
        }
    }
    pub(crate) fn health_for(&self, tool: &str) -> HealthDescriptor {
        match tool {
            "image_inspect" => HealthDescriptor::healthy("image", "builtin"),
            "image_generate" | "image_edit" => self
                .image
                .first()
                .map(|p| HealthDescriptor::healthy("image", &p.name))
                .unwrap_or_else(|| {
                    HealthDescriptor::unavailable(
                        "image",
                        "configure OPENAI_API_KEY or toolServices.image.apiKey",
                    )
                }),
            "tts" => self
                .tts
                .first()
                .map(|p| HealthDescriptor::healthy("tts", &p.name))
                .unwrap_or_else(|| {
                    HealthDescriptor::unavailable(
                        "tts",
                        "configure OPENAI_API_KEY or toolServices.tts provider",
                    )
                }),
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
            "image_generate" | "image_edit" => self.image_request(tool, input, context),
            "tts" => self.tts_request(input, context),
            _ => Err(ServiceError::InvalidInput(format!(
                "unknown media tool {tool}"
            ))),
        }
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
    fn image_request(
        &self,
        tool: &str,
        input: &Value,
        context: &OperationContext<'_>,
    ) -> ServiceResult<Value> {
        check_operation(context)?;
        let providers = &self.image;
        if providers.is_empty() {
            return Err(ServiceError::Unavailable {
                service: "image",
                detail: self.health_for(tool).detail,
            });
        }
        let prompt = nonempty(input.get("prompt"), "prompt")?;
        let mut body = json!({"model":providers[0].model,"prompt":prompt,"size":input.get("size").and_then(Value::as_str).unwrap_or("1024x1024"),"response_format":"b64_json"});
        if tool == "image_edit" {
            let path = self.jailed(input)?;
            let bytes = fs::read(path)?;
            if bytes.len() > MAX_INPUT_IMAGE {
                return Err(ServiceError::OutputLimit {
                    limit: MAX_INPUT_IMAGE,
                });
            }
            body["image"] = json!(base64::engine::general_purpose::STANDARD.encode(bytes));
        }
        let response = post_json_fallback(providers, &body, context, "image")?;
        let encoded = response
            .pointer("/data/0/b64_json")
            .and_then(Value::as_str)
            .ok_or_else(|| ServiceError::Protocol {
                service: "image",
                detail: "provider response lacks data[0].b64_json".into(),
            })?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|e| ServiceError::Protocol {
                service: "image",
                detail: e.to_string(),
            })?;
        let mut artifact = write_media_artifact(context, "image", "png", &bytes)?;
        artifact["provider"] = response.get("_provider").cloned().unwrap_or(Value::Null);
        artifact["revisedPrompt"] = response
            .pointer("/data/0/revised_prompt")
            .cloned()
            .unwrap_or(Value::Null);
        Ok(artifact)
    }
    fn tts_request(&self, input: &Value, context: &OperationContext<'_>) -> ServiceResult<Value> {
        check_operation(context)?;
        let provider = self.tts.first().ok_or_else(|| ServiceError::Unavailable {
            service: "tts",
            detail: self.health_for("tts").detail,
        })?;
        let text = nonempty(input.get("text"), "text")?;
        if text.len() > 32_000 {
            return Err(ServiceError::OutputLimit { limit: 32_000 });
        }
        let format = input.get("format").and_then(Value::as_str).unwrap_or("mp3");
        if !matches!(format, "mp3" | "wav" | "opus" | "aac" | "flac") {
            return Err(ServiceError::InvalidInput("unsupported TTS format".into()));
        }
        let client = Client::builder()
            .timeout(timeout(context))
            .build()
            .map_err(|e| ServiceError::Backend {
                service: "tts",
                detail: e.to_string(),
            })?;
        let response=client.post(&provider.endpoint).bearer_auth(&provider.key).json(&json!({"model":provider.model,"input":text,"voice":input.get("voice").and_then(Value::as_str).unwrap_or("alloy"),"response_format":format})).send().and_then(|r|r.error_for_status()).map_err(|e|ServiceError::Backend{service:"tts",detail:e.to_string()})?;
        let bytes = response.bytes().map_err(|e| ServiceError::Backend {
            service: "tts",
            detail: e.to_string(),
        })?;
        check_operation(context)?;
        let mut artifact = write_media_artifact(context, "tts", format, &bytes)?;
        artifact["provider"] = json!(provider.name);
        Ok(artifact)
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
            .map_err(|e| ServiceError::Io(e.to_string()))?;
        let root = self
            .cwd
            .canonicalize()
            .map_err(|e| ServiceError::Io(e.to_string()))?;
        if !canonical.starts_with(root) {
            return Err(ServiceError::PermissionDenied(
                "image path escapes workspace".into(),
            ));
        }
        Ok(canonical)
    }
}
fn post_json_fallback(
    providers: &[Provider],
    body: &Value,
    context: &OperationContext<'_>,
    service: &'static str,
) -> ServiceResult<Value> {
    let client = Client::builder()
        .timeout(timeout(context))
        .build()
        .map_err(|e| ServiceError::Backend {
            service,
            detail: e.to_string(),
        })?;
    let mut failures = Vec::new();
    for provider in providers {
        check_operation(context)?;
        match client
            .post(&provider.endpoint)
            .bearer_auth(&provider.key)
            .json(body)
            .send()
            .and_then(|r| r.error_for_status())
            .and_then(|r| r.json::<Value>())
        {
            Ok(mut value) => {
                value["_provider"] = json!(provider.name);
                return Ok(value);
            }
            Err(error) => failures.push(format!("{}: {error}", provider.name)),
        }
    }
    Err(ServiceError::Backend {
        service,
        detail: failures.join("; "),
    })
}
fn timeout(context: &OperationContext<'_>) -> Duration {
    context
        .deadline()
        .and_then(|d| d.checked_duration_since(std::time::Instant::now()))
        .unwrap_or(Duration::from_secs(90))
        .min(Duration::from_secs(120))
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
