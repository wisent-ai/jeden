use std::path::Path;

use crate::capability::{self, CapabilityKind, HealthState};

use super::attachments::ClipboardContent;

// Only the macOS build reads the clipboard, so elsewhere nothing asks for the
// two clipboard features.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiFeature {
    ClipboardRead,
    ClipboardImage,
    Steering,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeatureAvailability {
    pub available: bool,
    pub reason: Option<String>,
}

impl FeatureAvailability {
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            available: false,
            reason: Some(reason.into()),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeStatus {
    pub generation: u64,
    pub services_healthy: usize,
    pub services_degraded: usize,
    pub services_unavailable: usize,
    pub route_health: Option<String>,
    pub active_jobs: Option<u64>,
}

pub trait UiRuntimeAdapter: Send + Sync {
    fn availability(&self, cwd: &Path, feature: UiFeature) -> FeatureAvailability;
    fn read_clipboard(&self, cwd: &Path) -> Result<Option<ClipboardContent>, String>;
    fn runtime_status(&self, cwd: &Path) -> RuntimeStatus;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct RegistryUiRuntime;

impl UiRuntimeAdapter for RegistryUiRuntime {
    fn availability(&self, cwd: &Path, feature: UiFeature) -> FeatureAvailability {
        let operation = match feature {
            UiFeature::ClipboardRead => "clipboard-read",
            UiFeature::ClipboardImage => "clipboard-image",
            UiFeature::Steering => "steer-active-turn",
        };
        let snapshot = capability::for_cwd(cwd);
        let mut reasons = Vec::new();
        #[cfg(target_os = "macos")]
        let mut described = false;
        for descriptor in snapshot.descriptors.iter().filter(|descriptor| {
            descriptor
                .operations
                .iter()
                .any(|candidate| candidate == operation)
        }) {
            #[cfg(target_os = "macos")]
            {
                described = true;
            }
            if descriptor.ui.executable && descriptor.health.is_executable() {
                return FeatureAvailability {
                    available: true,
                    reason: None,
                };
            }
            if let Some(detail) = &descriptor.health.detail {
                reasons.push(detail.clone());
            }
        }
        #[cfg(target_os = "macos")]
        if !described
            && matches!(
                feature,
                UiFeature::ClipboardRead | UiFeature::ClipboardImage
            )
        {
            return FeatureAvailability {
                available: true,
                reason: None,
            };
        }
        let reason = if reasons.is_empty() {
            format!("No executable capability provides operation `{operation}`")
        } else {
            reasons.join("; ")
        };
        FeatureAvailability::unavailable(reason)
    }

    fn read_clipboard(&self, cwd: &Path) -> Result<Option<ClipboardContent>, String> {
        #[cfg(target_os = "macos")]
        {
            let image = self.availability(cwd, UiFeature::ClipboardImage);
            let text = self.availability(cwd, UiFeature::ClipboardRead);
            let mut image_error = None;
            if image.available {
                match clipboard_image() {
                    Ok(bytes) => {
                        return Ok(Some(ClipboardContent::from((
                            "clipboard.png".into(),
                            bytes,
                        ))))
                    }
                    Err(error) => image_error = Some(error),
                }
            }
            if text.available {
                let output = stado_wait::blocking(
                    stado_wait::Kind::Process,
                    "pbpaste reading the clipboard text",
                    "pbpaste",
                    || std::process::Command::new("pbpaste").output(),
                )
                .map_err(|error| format!("Clipboard text read failed: {error}"))?;
                if !output.status.success() {
                    return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
                }
                if output.stdout.is_empty() {
                    return Ok(None);
                }
                return String::from_utf8(output.stdout)
                    .map(|value| Some(ClipboardContent::from(value)))
                    .map_err(|error| format!("Clipboard text is not UTF-8: {error}"));
            }
            // Neither read could be made: every cause there is, in order.
            let causes: Vec<String> = [image_error, image.reason, text.reason]
                .into_iter()
                .flatten()
                .collect();
            match causes.as_slice() {
                [] => Err("neither the clipboard image nor its text can be read, and no capability says why".into()),
                _ => Err(causes.join("; ")),
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(format!(
                "No native clipboard adapter is available on this platform for {}",
                cwd.display()
            ))
        }
    }

    fn runtime_status(&self, cwd: &Path) -> RuntimeStatus {
        let snapshot = capability::for_cwd(cwd);
        let services: Vec<_> = snapshot.kind(CapabilityKind::Service).collect();
        let counted = |states: &[HealthState]| {
            services
                .iter()
                .filter(|descriptor| states.contains(&descriptor.health.state))
                .count()
        };
        let mut status = RuntimeStatus {
            generation: snapshot.generation,
            services_healthy: counted(&[HealthState::Healthy]),
            services_degraded: counted(&[HealthState::Degraded]),
            services_unavailable: counted(&[HealthState::Unavailable, HealthState::Disabled]),
            ..RuntimeStatus::default()
        };
        for descriptor in &services {
            if descriptor
                .operations
                .iter()
                .any(|operation| operation == "route-health")
            {
                status.route_health = Some(
                    match descriptor.health.state {
                        HealthState::Healthy => "healthy",
                        HealthState::Degraded => "degraded",
                        HealthState::Unavailable => "unavailable",
                        HealthState::Disabled => "disabled",
                    }
                    .to_string(),
                );
            }
            if let Some(running) = descriptor
                .metadata
                .get("runningJobs")
                .and_then(|value| value.as_u64())
            {
                status.active_jobs = Some(running);
            }
        }
        status
    }
}

/// The clipboard's image as PNG bytes, read from the data AppleScript itself
/// answers (`«data PNGf<hex>»`), so no copy of what the operator copied is
/// written to any file. A clipboard holding no image is osascript's refusal.
#[cfg(target_os = "macos")]
fn clipboard_image() -> Result<Vec<u8>, String> {
    let output = stado_wait::blocking(
        stado_wait::Kind::Process,
        "osascript reading the clipboard image",
        "osascript",
        || {
            std::process::Command::new("osascript")
                .args(["-e", "the clipboard as «class PNGf»"])
                .output()
        },
    )
    .map_err(|error| format!("Clipboard image read failed: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let answered = String::from_utf8(output.stdout).map_err(|error| {
        format!("osascript answered the clipboard image in text that is not UTF-8: {error}")
    })?;
    let hex = answered
        .trim()
        .strip_prefix("«data PNGf")
        .and_then(|rest| rest.strip_suffix('»'))
        .ok_or_else(|| {
            "osascript answered the clipboard image in a form other than «data PNGf…»".to_string()
        })?;
    hex::decode(hex).map_err(|error| {
        format!("the clipboard image osascript answered is not hexadecimal: {error}")
    })
}
