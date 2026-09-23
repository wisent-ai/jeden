use super::super::types::{check_operation, nonempty, write_media_artifact, ServiceError, ServiceResult};
use super::formats::{
    aspect_ratio, dimensions, image_extension, image_metadata, optional_string, validate_submission,
};
use super::router::check_job_id;
use super::{
    CapabilityRequest, CapabilitySubmission, EncodedMediaSample, ImageGenerateRequest,
    ImageGenerateResponse, MediaService, MAX_INPUT_IMAGE,
};
use crate::tool_runtime::runtime_ops::OperationContext;
use base64::Engine;
use serde_json::{json, Value};
use std::fs;
use std::time::Duration;

/// The operations that go through the Stado media router.
impl MediaService {
    pub(super) fn image_generate(
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

    pub(super) fn image_edit(&self, input: &Value, context: &OperationContext<'_>) -> ServiceResult<Value> {
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

    pub(super) fn tts_request(&self, input: &Value, context: &OperationContext<'_>) -> ServiceResult<Value> {
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

    pub(super) fn wait_for_completion(
        &self,
        job_id: &str,
        context: &OperationContext<'_>,
    ) -> ServiceResult<()> {
        // The router reports the job as completed, failed or cancelled; those
        // are the ends of this wait. A cancelled turn stops it too.
        loop {
            check_operation(context)?;
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
}
