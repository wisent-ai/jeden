//! First use: the guided journey a new operator is taken through, and what
//! marks each step done.

use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;
use wisent_onboarding_client::{FileStorage, JourneyClient, ProgressStatus, ScopeKind, Transport};

use crate::tui::{CommandOutcome, PickerSpec};

mod client;
mod screen;

use client::{run, start_client, state_path, subject_hash};
use screen::{current_screen, picker_for, presentation_text};

const EVIDENCE_REVISION: &str = "jeden-first-use-2026-09-05";
const JOURNEY_VERSION_ID: &str = "10000000-0000-4000-8000-000000000005";
const FALLBACK: &str = include_str!("first_use.json");

type Client = JourneyClient<Box<dyn Transport>, FileStorage>;

async fn apply(action: &str, cwd: &Path) -> Result<(Client, Option<String>), String> {
    let mut client = start_client().await?;
    let mut notice = None;
    let (verb, rest) = action
        .trim()
        .split_once(char::is_whitespace)
        .map(|(verb, rest)| (verb, rest.trim()))
        .unwrap_or((action.trim(), ""));
    match verb {
        "" | "show" => {}
        "next" => {
            client
                .advance(&BTreeMap::new(), EVIDENCE_REVISION)
                .await
                .map_err(|error| error.to_string())?;
        }
        "skip" => {
            if current_screen(&client)?.screen_kind == "source_selection" {
                client
                    .advance(&BTreeMap::new(), EVIDENCE_REVISION)
                    .await
                    .map_err(|error| error.to_string())?
                    .ok_or_else(|| "onboarding workspace step could not advance".to_string())?;
            } else {
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
        }
        "reset" => {
            client
                .reset(EVIDENCE_REVISION)
                .await
                .map_err(|error| error.to_string())?;
        }
        "adopt-workspace" => {
            let path = if rest.is_empty() {
                cwd
            } else {
                Path::new(rest)
            };
            let report = crate::cli::workspace::adopt(path, cwd)?;
            let evidence = BTreeMap::from([("workspace_adopted".to_string(), json!(true))]);
            client
                .advance(&evidence, EVIDENCE_REVISION)
                .await
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "onboarding workspace step could not advance".to_string())?;
            notice = Some(format!(
                "{}: {} · {} existing session(s) accepted",
                if report.status == "unchanged" {
                    "Already using"
                } else {
                    "Adopted"
                },
                report.workspace.display(),
                report.sessions.accepted
            ));
        }
        _ => return Err("usage: /onboarding [next|skip|reset|adopt-workspace <path>]".into()),
    }
    client
        .expose(EVIDENCE_REVISION)
        .await
        .map_err(|error| error.to_string())?;
    Ok((client, notice))
}

pub(crate) fn initial_picker(cwd: &Path) -> Result<Option<PickerSpec>, String> {
    run(async {
        let client = start_client().await?;
        if client
            .progress()
            .is_some_and(|progress| progress.status == ProgressStatus::Completed)
        {
            return Ok(None);
        }
        client
            .expose(EVIDENCE_REVISION)
            .await
            .map_err(|error| error.to_string())?;
        picker_for(&client, cwd, None).map(Some)
    })
}

pub(crate) fn interactive(action: &str, cwd: &Path) -> Result<CommandOutcome, String> {
    run(async {
        let (client, notice) = apply(action.trim(), cwd).await?;
        picker_for(&client, cwd, notice.as_deref()).map(CommandOutcome::Picker)
    })
}

pub(crate) fn text(action: &str, cwd: &Path) -> Result<String, String> {
    interactive(action, cwd).map(CommandOutcome::into_text)
}

pub(crate) fn observe_successful_turn() {
    let _ = run(async {
        let mut client = start_client().await?;
        if client
            .progress()
            .is_some_and(|progress| progress.status == ProgressStatus::Completed)
        {
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
