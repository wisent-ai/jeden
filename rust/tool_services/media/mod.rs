mod formats;
mod jobs;
mod router;

use super::config;
use super::types::{
    bounded_json, check_operation, nonempty, HealthDescriptor, ServiceError, ServiceResult,
};
use crate::tool_runtime::runtime_ops::OperationContext;
use formats::image_metadata;
use router::MediaRouterClient;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
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
