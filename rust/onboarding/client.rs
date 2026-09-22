//! Opening the onboarding journey, offline when there is nowhere to ask.
//!
//! Split out of `onboarding/mod.rs`, which had grown past the module line cap.

use super::{Client, EVIDENCE_REVISION, FALLBACK, JOURNEY_VERSION_ID};
use sha2::{Digest, Sha256};
use std::env;
use std::future::Future;
use std::path::PathBuf;
use uuid::Uuid;
use wisent_onboarding_client::ScopeKind;
use wisent_onboarding_client::{
    bundle_from_canonical, FileStorage, IntegrationTransport, JourneyClient, Transport,
};

pub(super) fn state_path() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".jeden/onboarding-state.json")
}

pub(super) fn subject_hash() -> String {
    let mut digest = Sha256::new();
    digest.update(env::var("USER").unwrap_or_else(|_| "unknown-user".into()));
    digest.update(b"\0jeden-first-use\0");
    digest.update(
        env::var_os("HOME")
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_default(),
    );
    hex::encode(digest.finalize())
}

fn transport() -> Box<dyn Transport> {
    let endpoint = env::var("STADO_INTEGRATION_API_URL").unwrap_or_default();
    let token = env::var("JEDEN_STADO_INTEGRATION_TOKEN").unwrap_or_default();
    if !endpoint.trim().is_empty() && !token.trim().is_empty() {
        if let Ok(transport) = IntegrationTransport::new(endpoint.trim(), token) {
            return Box::new(transport);
        }
    }
    Box::new(wisent_onboarding_client::OfflineTransport)
}

pub(super) async fn start_client() -> Result<Client, String> {
    let fallback = bundle_from_canonical(
        FALLBACK,
        Uuid::parse_str(JOURNEY_VERSION_ID).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    let mut client = JourneyClient::new(
        "jeden",
        "first-use",
        subject_hash(),
        ScopeKind::Device,
        transport(),
        FileStorage::new(state_path()),
        fallback,
    )
    .map_err(|error| error.to_string())?;
    client
        .start(EVIDENCE_REVISION)
        .await
        .map_err(|error| error.to_string())?;
    let _ = client.flush().await;
    Ok(client)
}

pub(super) fn run<T>(future: impl Future<Output = Result<T, String>>) -> Result<T, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?
        .block_on(future)
}
