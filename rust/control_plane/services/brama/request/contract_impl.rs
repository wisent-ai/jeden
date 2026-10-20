//! The versioned Brama API contract, separate from client construction and transport.
use super::{BramaClient, BramaError, ModelCatalog, ModelEntry};
use crate::control_plane::contract::{
    self, BramaApiV1, ModelRequest, ModelStreamResultV1, RequestMeta, RouteRequest,
};
use crate::control_plane::ServiceHealth;

/// BramaApiV1 speaks only the first supported control-plane schema.
const BRAMA_V1_SCHEMA_VERSION: u32 = contract::MIN_SCHEMA_VERSION;

impl BramaApiV1 for BramaClient {
    fn health(&self) -> ServiceHealth {
        BramaClient::health(self)
    }

    fn readiness(&self) -> Result<contract::Readiness, BramaError> {
        let catalog = self.catalog(false)?;
        contract::negotiate(BRAMA_V1_SCHEMA_VERSION, BRAMA_V1_SCHEMA_VERSION).map_err(|error| {
            BramaError::InvalidCatalog(format!("schema negotiation failed: {error:?}"))
        })?;
        Ok(contract::Readiness {
            ready: true,
            schema_min: BRAMA_V1_SCHEMA_VERSION,
            schema_max: BRAMA_V1_SCHEMA_VERSION,
            detail: format!(
                "{} routes available",
                catalog
                    .models
                    .iter()
                    .filter(|model| model.available)
                    .count()
            ),
        })
    }

    fn capabilities(&self, meta: &RequestMeta) -> Result<Vec<String>, BramaError> {
        let response = self.request_json(reqwest::Method::GET, "/capabilities", None, meta)?;
        let value: serde_json::Value = serde_json::from_slice(&response.body)
            .map_err(|error| BramaError::InvalidResponse(error.to_string()))?;
        let capabilities = match value {
            value @ serde_json::Value::Array(_) => value,
            serde_json::Value::Object(mut fields) => {
                fields.remove("capabilities").ok_or_else(|| {
                    BramaError::InvalidResponse("GET /capabilities omitted capabilities".into())
                })?
            }
            _ => {
                return Err(BramaError::InvalidResponse(
                    "GET /capabilities must answer an array or an object with capabilities".into(),
                ))
            }
        };
        serde_json::from_value(capabilities)
            .map_err(|error| BramaError::InvalidResponse(error.to_string()))
    }

    fn catalog(&self, force: bool) -> Result<ModelCatalog, BramaError> {
        BramaClient::catalog(self, force)
    }

    fn resolve(
        &self,
        request: &RouteRequest,
        meta: &RequestMeta,
    ) -> Result<ModelEntry, BramaError> {
        let body = serde_json::to_vec(request)
            .map_err(|error| BramaError::InvalidResponse(error.to_string()))?;
        let response = self.request_json(reqwest::Method::POST, "/resolve", Some(body), meta)?;
        let route: ModelEntry = serde_json::from_slice(&response.body)
            .map_err(|error| BramaError::InvalidResponse(error.to_string()))?;
        if route.id.trim().is_empty() {
            return Err(BramaError::InvalidResponse("served route is empty".into()));
        }
        Ok(route)
    }

    fn stream(
        &self,
        request: &ModelRequest,
        meta: &RequestMeta,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<ModelStreamResultV1, BramaError> {
        if cancelled() {
            return Err(BramaError::Cancelled);
        }
        let body = serde_json::to_vec(request)
            .map_err(|error| BramaError::InvalidResponse(error.to_string()))?;
        let response = self.request_json(reqwest::Method::POST, "/stream", Some(body), meta)?;
        if cancelled() {
            return Err(BramaError::Cancelled);
        }
        let mut result: ModelStreamResultV1 = serde_json::from_slice(&response.body)
            .map_err(|error| BramaError::InvalidResponse(error.to_string()))?;
        if result.served_route.trim().is_empty()
            || result.finish_reason.trim().is_empty()
            || result.correlation_id != meta.correlation_id
        {
            return Err(BramaError::InvalidResponse(
                "stream terminal metadata is incomplete or correlation mismatched".into(),
            ));
        }
        if result.selected_route.is_empty() {
            result.selected_route = request.route.clone();
        }
        if result.selected_route != request.route {
            return Err(BramaError::InvalidResponse(
                "selected route does not match the requested route".into(),
            ));
        }
        if let Some(snapshot) = &result.billing {
            if snapshot.provider_id.is_empty()
                || snapshot.account_id.is_empty()
                || snapshot.subscription_id.is_empty()
                || snapshot.quota.subscription_id != snapshot.subscription_id
            {
                return Err(BramaError::InvalidResponse(
                    "billing attribution is incomplete or mismatched".into(),
                ));
            }
        }
        Ok(result)
    }
}
