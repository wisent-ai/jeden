use wisent_onboarding_client::{
    bundle_from_canonical, FileStorage, JourneyClient, ProgressStatus, ScopeKind,
    StadoTransport, Transport,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::env;
use std::future::Future;
use std::path::PathBuf;
use uuid::Uuid;

use crate::tui::{CommandOutcome, PickerItem, PickerSpec};

const EVIDENCE_REVISION: &str = "jeden-first-use-2026-08-04";
const JOURNEY_VERSION_ID: &str = "10000000-0000-4000-8000-000000000005";
const FALLBACK: &str = include_str!("onboarding_first_use.json");

type Client = JourneyClient<Box<dyn Transport>, FileStorage>;

fn state_path() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".jeden/onboarding-state.json")
}

fn subject_hash() -> String {
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
        if let Ok(transport) = StadoTransport::new(endpoint.trim(), token) {
            return Box::new(transport);
        }
    }
    Box::new(wisent_onboarding_client::OfflineTransport)
}

async fn start_client() -> Result<Client, String> {
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

fn run<T>(future: impl Future<Output = Result<T, String>>) -> Result<T, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?
        .block_on(future)
}

fn current_screen(client: &Client) -> Result<&wisent_onboarding_client::Screen, String> {
    let progress = client
        .progress()
        .ok_or_else(|| "onboarding progress is unavailable".to_string())?;
    client
        .bundle()
        .and_then(|bundle| {
            bundle
                .definition
                .screens
                .iter()
                .find(|screen| screen.screen_id == progress.current_screen_id)
        })
        .ok_or_else(|| "onboarding screen is unavailable".to_string())
}

fn presentation_text(screen: &wisent_onboarding_client::Screen, key: &str) -> String {
    screen
        .presentation
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn picker_for(client: &Client) -> Result<PickerSpec, String> {
    let progress = client
        .progress()
        .ok_or_else(|| "onboarding progress is unavailable".to_string())?;
    if progress.status == ProgressStatus::Completed {
        return Ok(PickerSpec::new(
            "Jeden first-use complete",
            vec![PickerItem::action("Replay the product guide", "/onboarding reset")
                .detail("starts a new first-use attempt without changing setup")
                .badge("REPLAY")],
        ));
    }

    let screen = current_screen(client)?;
    let title = presentation_text(screen, "title");
    let body = presentation_text(screen, "body");
    let mut items = vec![PickerItem::action(body, "").disabled(true)];
    if screen.transitions.is_empty() {
        items.push(
            PickerItem::action("Continue in the prompt", "")
                .detail("press Esc, then describe one real coding task")
                .badge("FIRST RESULT")
                .disabled(true),
        );
        items.push(
            PickerItem::action("Configure model access", "/setup")
                .detail("open the separate setup flow only if no model route is connected")
                .badge("SETUP"),
        );
    } else {
        items.push(
            PickerItem::action("Continue", "/onboarding next")
                .detail("show the next product concept"),
        );
        items.push(
            PickerItem::action("Skip explanation", "/onboarding skip")
                .detail("continue to the first real task; setup is unchanged"),
        );
    }
    Ok(PickerSpec::new(title, items))
}

async fn apply(action: &str) -> Result<Client, String> {
    let mut client = start_client().await?;
    match action {
        "" | "show" => {}
        "next" => {
            client
                .advance(&BTreeMap::new(), EVIDENCE_REVISION)
                .await
                .map_err(|error| error.to_string())?;
        }
        "skip" => {
            client
                .skip(EVIDENCE_REVISION)
                .await
                .map_err(|error| error.to_string())?;
            while !current_screen(&client)?.transitions.is_empty() {
                if client
                    .advance(&BTreeMap::new(), EVIDENCE_REVISION)
                    .await
                    .map_err(|error| error.to_string())?
                    .is_none()
                {
                    break;
                }
            }
            client
                .resume(EVIDENCE_REVISION)
                .await
                .map_err(|error| error.to_string())?;
        }
        "reset" => {
            client
                .reset(EVIDENCE_REVISION)
                .await
                .map_err(|error| error.to_string())?;
        }
        _ => return Err("usage: /onboarding [next|skip|reset]".into()),
    }
    client
        .expose(EVIDENCE_REVISION)
        .await
        .map_err(|error| error.to_string())?;
    Ok(client)
}

pub(crate) fn initial_picker() -> Result<Option<PickerSpec>, String> {
    run(async {
        let client = start_client().await?;
        if client.progress().is_some_and(|progress| progress.status == ProgressStatus::Completed) {
            return Ok(None);
        }
        client
            .expose(EVIDENCE_REVISION)
            .await
            .map_err(|error| error.to_string())?;
        picker_for(&client).map(Some)
    })
}

pub(crate) fn interactive(action: &str) -> Result<CommandOutcome, String> {
    run(async {
        let client = apply(action.trim()).await?;
        picker_for(&client).map(CommandOutcome::Picker)
    })
}

pub(crate) fn text(action: &str) -> Result<String, String> {
    interactive(action).map(CommandOutcome::into_text)
}

pub(crate) fn observe_successful_turn() {
    let _ = run(async {
        let mut client = start_client().await?;
        if client.progress().is_some_and(|progress| progress.status == ProgressStatus::Completed) {
            return Ok(());
        }
        while !current_screen(&client)?.transitions.is_empty() {
            if client
                .advance(&BTreeMap::new(), EVIDENCE_REVISION)
                .await
                .map_err(|error| error.to_string())?
                .is_none()
            {
                return Ok(());
            }
        }
        let evidence = BTreeMap::from([("successful_agent_turn".to_string(), json!(true))]);
        client
            .complete(&evidence, EVIDENCE_REVISION)
            .await
            .map_err(|error| error.to_string())?;
        Ok(())
    });
}
