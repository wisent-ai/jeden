//! Turning the current onboarding step into rows an operator can act on.
//!
//! Split out of `onboarding/mod.rs`, which had grown past the module line cap.

use super::Client;
use crate::tui::{PickerItem, PickerSpec};
use std::path::Path;
use wisent_onboarding_client::ProgressStatus;

pub(super) fn current_screen(client: &Client) -> Result<&wisent_onboarding_client::Screen, String> {
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

pub(super) fn presentation_text(screen: &wisent_onboarding_client::Screen, key: &str) -> String {
    screen
        .presentation
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

pub(super) fn picker_for(client: &Client, cwd: &Path, notice: Option<&str>) -> Result<PickerSpec, String> {
    let progress = client
        .progress()
        .ok_or_else(|| "onboarding progress is unavailable".to_string())?;
    if progress.status == ProgressStatus::Completed {
        return Ok(PickerSpec::new(
            "Jeden first-use complete",
            vec![
                PickerItem::action("Replay the product guide", "/onboarding reset")
                    .detail("starts a new first-use attempt without changing setup")
                    .badge("REPLAY"),
            ],
        ));
    }

    let screen = current_screen(client)?;
    let title = presentation_text(screen, "title");
    let body = presentation_text(screen, "body");
    let mut items = vec![PickerItem::action(body, "").disabled(true)];
    if let Some(notice) = notice {
        items.push(
            PickerItem::action(notice, "")
                .badge("ACCEPTED")
                .disabled(true),
        );
    }
    if screen
        .actions
        .iter()
        .any(|action| action == "adopt_workspace")
    {
        items.push(
            PickerItem::action(
                format!("Adopt {}", cwd.display()),
                format!("/onboarding adopt-workspace {}", cwd.display()),
            )
            .detail("validate and select this existing workspace without copying it")
            .badge("USE EXISTING"),
        );
        items.push(
            PickerItem::action(
                "Choose another workspace path",
                "/onboarding adopt-workspace ",
            )
            .detail("enter an existing readable repository or directory")
            .badge("INPUT")
            .prefill(),
        );
        items.push(
            PickerItem::action("Skip for now", "/onboarding skip")
                .detail("keep the current directory for this run; nothing is persisted"),
        );
        return Ok(PickerSpec::new(title, items));
    }
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
