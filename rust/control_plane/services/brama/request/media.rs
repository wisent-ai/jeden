//! Media Brama generates itself: one image on `POST /v1/images/generations`
//! and one spoken text on `POST /v1/audio/speech`. Both answer in the same
//! request, so nothing here starts a job or reads one back.

use super::super::contract::RequestMeta;
use super::{BramaClient, BramaError};
use base64::Engine;
use serde_json::{json, Value};

/// The largest generated image or spoken text Jeden accepts from Brama.
const MAX_MEDIA_RESPONSE_BYTES: u64 = 64 * 1024 * 1024;

impl BramaClient {
    /// One image from `model` (the deployment's `image-model` alias or a
    /// canonical image route). `size` is the provider's `WIDTHxHEIGHT`.
    pub fn generate_image(
        &self,
        model: &str,
        prompt: &str,
        size: Option<&str>,
        meta: &RequestMeta,
    ) -> Result<Vec<u8>, BramaError> {
        let mut request = json!({
            "model": model,
            "prompt": prompt,
            "n": 1,
            "response_format": "b64_json",
        });
        if let Some(size) = size {
            request["size"] = json!(size);
        }
        let body = serde_json::to_vec(&request)
            .map_err(|error| BramaError::InvalidResponse(error.to_string()))?;
        let response = self.request_bounded(
            reqwest::Method::POST,
            "/images/generations",
            Some(body),
            meta,
            MAX_MEDIA_RESPONSE_BYTES,
        )?;
        let value: Value = serde_json::from_slice(&response.body).map_err(|error| {
            BramaError::InvalidResponse(format!("image response is not JSON: {error}"))
        })?;
        let encoded = value
            .get("data")
            .and_then(Value::as_array)
            .and_then(|images| images.first())
            .and_then(|image| image.get("b64_json"))
            .and_then(Value::as_str)
            .ok_or_else(|| {
                BramaError::InvalidResponse("image response carries no data[0].b64_json".into())
            })?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|error| BramaError::InvalidResponse(format!("invalid b64_json: {error}")))?;
        if bytes.is_empty() {
            return Err(BramaError::InvalidResponse(
                "image response decoded to nothing".into(),
            ));
        }
        Ok(bytes)
    }

    /// `input` spoken by `voice` through `model` (the deployment's
    /// `voice-model` alias or a canonical speech route), as the provider's
    /// encoded audio with the content type Brama stated.
    pub fn speak(
        &self,
        model: &str,
        input: &str,
        voice: &str,
        response_format: &str,
        meta: &RequestMeta,
    ) -> Result<(Vec<u8>, String), BramaError> {
        let body = serde_json::to_vec(&json!({
            "model": model,
            "input": input,
            "voice": voice,
            "response_format": response_format,
        }))
        .map_err(|error| BramaError::InvalidResponse(error.to_string()))?;
        let response = self.request_bounded(
            reqwest::Method::POST,
            "/audio/speech",
            Some(body),
            meta,
            MAX_MEDIA_RESPONSE_BYTES,
        )?;
        let content_type = response
            .headers
            .get("content-type")
            .cloned()
            .filter(|value| value.starts_with("audio/"))
            .ok_or_else(|| BramaError::InvalidResponse("speech response is not audio".into()))?;
        if response.body.is_empty() {
            return Err(BramaError::InvalidResponse(
                "speech response is empty".into(),
            ));
        }
        Ok((response.body, content_type))
    }
}
