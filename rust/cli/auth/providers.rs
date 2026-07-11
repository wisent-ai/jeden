//! Legacy provider config shape retained for config-file compatibility.
//!
//! Jeden no longer performs provider OAuth or stores resulting credentials;
//! entitlements-router owns authentication resolution.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct AuthProviderConfig {
    #[serde(rename = "authUrl")]
    auth_url: Option<String>,
    #[serde(rename = "authorizeUrl")]
    authorize_url: Option<String>,
    #[serde(rename = "tokenUrl")]
    token_url: Option<String>,
    #[serde(rename = "clientId")]
    client_id: Option<String>,
    #[serde(rename = "redirectUri")]
    redirect_uri: Option<String>,
    scope: Option<String>,
    open: Option<bool>,
}
