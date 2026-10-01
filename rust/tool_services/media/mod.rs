mod formats;
mod jobs;

use super::config;
use super::types::{
    bounded_json, check_operation, nonempty, HealthDescriptor, ServiceError, ServiceResult,
};
use crate::control_plane::brama::BramaClient;
use crate::tool_runtime::runtime_ops::OperationContext;
use formats::image_metadata;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
const MAX_INPUT_IMAGE: usize = 20 * 1024 * 1024;
/// The deployment's media aliases in Brama; a configured model overrides each.
const IMAGE_ALIAS: &str = "image-model";
const VOICE_ALIAS: &str = "voice-model";
pub(crate) const TOOLS: &[(&str, &str)] = &[
    (
        "image_inspect",
        "Inspect image format, dimensions, size, and digest",
    ),
    (
        "image_generate",
        "Generate an image through Brama's image model and preserve it as an artifact",
    ),
    (
        "tts",
        "Synthesize speech through Brama's voice model and preserve it as an artifact",
    ),
];

pub(crate) struct MediaService {
    cwd: PathBuf,
    brama: BramaClient,
    image_model: String,
    tts_model: String,
}

impl MediaService {
    pub(crate) fn discover(cwd: &Path, value: &Value) -> Self {
        Self {
            cwd: cwd.to_path_buf(),
            brama: BramaClient::from_env(),
            image_model: config::string(
                value,
                &["toolServices", "image", "model"],
                "JEDEN_IMAGE_MODEL",
            )
            .unwrap_or_else(|| IMAGE_ALIAS.into()),
            tts_model: config::string(value, &["toolServices", "tts", "model"], "JEDEN_TTS_MODEL")
                .unwrap_or_else(|| VOICE_ALIAS.into()),
        }
    }

    pub(crate) fn health_for(&self, tool: &str) -> HealthDescriptor {
        match tool {
            "image_inspect" => HealthDescriptor::healthy("image", "builtin"),
            "image_generate" | "tts" => {
                let brama = self.brama.health();
                if brama.available {
                    HealthDescriptor::healthy("media", "brama")
                } else {
                    HealthDescriptor::unavailable("media", brama.detail)
                }
            }
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
