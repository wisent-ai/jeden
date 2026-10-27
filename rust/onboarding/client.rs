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

/// The onboarding state under the user's home, resolved by the one home
/// lookup: read against `.`, every project Jeden started in got its own state.
pub(super) fn state_path() -> PathBuf {
    crate::slash::common::dirs_home().join(".jeden/onboarding-state.json")
}

/// The device subject onboarding progress is kept for: the user name and the
/// home it runs under. Without a user name there is no subject to keep it
/// for; a placeholder made every nameless account share one journey.
pub(super) fn subject_hash() -> Result<String, String> {
    let user = env::var("USER").map_err(|error| {
        format!("USER cannot be read, so onboarding has no subject to keep progress for: {error}")
    })?;
    let mut digest = Sha256::new();
    digest.update(user);
    digest.update(b"\0jeden-first-use\0");
    digest.update(
        crate::slash::common::dirs_home()
            .to_string_lossy()
            .as_bytes(),
    );
    Ok(hex::encode(digest.finalize()))
}

/// A variable's declared value: none when it is unset or blank, refused when
/// it is not text.
fn declared(name: &str) -> Result<Option<String>, String> {
    match env::var(name) {
        Ok(value) if value.trim().is_empty() => Ok(None),
        Ok(value) => Ok(Some(value.trim().to_string())),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(format!("{name} cannot be read: {error}")),
    }
}

/// Where onboarding progress is reported: the Stado integration API when both
/// its address and token are declared, offline when neither is. One without
/// the other, or an address the transport refuses, is refused by name: read
/// as offline, a misconfigured integration never reported anything.
fn transport() -> Result<Box<dyn Transport>, String> {
    let endpoint = declared("STADO_INTEGRATION_API_URL")?;
    let token = declared("JEDEN_STADO_INTEGRATION_TOKEN")?;
    match (endpoint, token) {
        (Some(endpoint), Some(token)) => IntegrationTransport::new(&endpoint, token)
            .map(|transport| Box::new(transport) as Box<dyn Transport>)
            .map_err(|error| format!("STADO_INTEGRATION_API_URL {endpoint}: {error}")),
        (Some(_), None) => Err(
            "STADO_INTEGRATION_API_URL is declared without JEDEN_STADO_INTEGRATION_TOKEN".into(),
        ),
        (None, Some(_)) => Err(
            "JEDEN_STADO_INTEGRATION_TOKEN is declared without STADO_INTEGRATION_API_URL".into(),
        ),
        (None, None) => Ok(Box::new(wisent_onboarding_client::OfflineTransport)),
    }
}

pub(super) async fn start_client() -> Result<Client, String> {
    let version = Uuid::parse_str(JOURNEY_VERSION_ID).map_err(|error| {
        format!("the first-use journey version {JOURNEY_VERSION_ID} is not a UUID: {error}")
    })?;
    let fallback = bundle_from_canonical(FALLBACK, version).map_err(|error| {
        format!("the first-use journey shipped with jeden (onboarding/first_use.json) cannot be read: {error}")
    })?;
    let state = state_path();
    let mut client = JourneyClient::new(
        "jeden",
        "first-use",
        subject_hash()?,
        ScopeKind::Device,
        transport()?,
        FileStorage::new(state.clone()),
        fallback,
    )
    .map_err(|error| {
        format!("the onboarding journey kept at {} cannot be opened: {error}", state.display())
    })?;
    client.start(EVIDENCE_REVISION).await.map_err(|error| {
        format!("the onboarding journey kept at {} cannot be started: {error}", state.display())
    })?;
    // Undelivered events stay queued in the client; only a storage failure
    // is an error here, and it is refused rather than dropped.
    client.flush().await.map_err(|error| {
        format!("the onboarding events kept at {} cannot be stored: {error}", state.display())
    })?;
    Ok(client)
}

pub(super) fn run<T>(future: impl Future<Output = Result<T, String>>) -> Result<T, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("the onboarding runtime cannot be started: {error}"))?
        .block_on(future)
}
