use super::super::types::{
    check_operation, nonempty, write_media_artifact, ServiceError, ServiceResult,
};
use super::formats::{dimensions, image_metadata, optional_string};
use super::MediaService;
use crate::control_plane::brama::BramaError;
use crate::control_plane::contract::RequestMeta;
use crate::tool_runtime::runtime_ops::OperationContext;
use serde_json::{json, Value};

/// Generation through Brama: one request, one answer, nothing to read back.
impl MediaService {
    pub(super) fn image_generate(
        &self,
        input: &Value,
        context: &OperationContext<'_>,
    ) -> ServiceResult<Value> {
        check_operation(context)?;
        let prompt = nonempty(input.get("prompt"), "prompt")?;
        let size = input.get("size").and_then(Value::as_str);
        dimensions(size)?;
        let model = match optional_string(input, "model") {
            Some(chosen) => chosen,
            None => self.image_model.clone(),
        };
        let bytes = self
            .brama
            .generate_image(&model, &prompt, size, &meta("image"))
            .map_err(brama_failure)?;
        let (format, width, height) = image_metadata(&bytes)?;
        let mut artifact = write_media_artifact(context, "image", format, &bytes)?;
        artifact["provider"] = json!("brama");
        artifact["model"] = json!(model);
        artifact["width"] = json!(width);
        artifact["height"] = json!(height);
        Ok(artifact)
    }

    pub(super) fn tts_request(
        &self,
        input: &Value,
        context: &OperationContext<'_>,
    ) -> ServiceResult<Value> {
        check_operation(context)?;
        let text = nonempty(input.get("text"), "text")?;
        let voice = optional_string(input, "voice")
            .or_else(|| optional_string(input, "voice_id"))
            .ok_or_else(|| ServiceError::InvalidInput("tts requires voice".into()))?;
        // A request that names no format is spoken as mp3, the first format
        // the tool's contract lists.
        let format = match optional_string(input, "format") {
            Some(format) => format,
            None => "mp3".to_string(),
        };
        if !matches!(format.as_str(), "mp3" | "wav" | "opus" | "aac" | "flac") {
            return Err(ServiceError::InvalidInput(format!(
                "TTS format {format} is not one of mp3, wav, opus, aac, flac"
            )));
        }
        let model = match optional_string(input, "model") {
            Some(chosen) => chosen,
            None => self.tts_model.clone(),
        };
        let (bytes, mime_type) = self
            .brama
            .speak(&model, &text, &voice, &format, &meta("speech"))
            .map_err(brama_failure)?;
        let mut artifact = write_media_artifact(context, "tts", &format, &bytes)?;
        artifact["provider"] = json!("brama");
        artifact["model"] = json!(model);
        artifact["mimeType"] = json!(mime_type);
        Ok(artifact)
    }
}

fn meta(kind: &str) -> RequestMeta {
    RequestMeta::read(format!("jeden-media-{kind}-{}", uuid::Uuid::new_v4()))
}

fn brama_failure(error: BramaError) -> ServiceError {
    match error {
        BramaError::Unconfigured => ServiceError::Unavailable {
            service: "brama",
            detail: error.to_string(),
        },
        other => ServiceError::Backend {
            service: "brama",
            detail: other.to_string(),
        },
    }
}
