use std::path::Path;
use std::sync::Arc;

use crate::capability::{self, CapabilityKind, HealthState};

use super::attachments::ClipboardContent;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiFeature {
    ClipboardRead,
    ClipboardImage,
    InlineImage,
    Steering,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeatureAvailability {
    pub available: bool,
    pub reason: Option<String>,
}

impl FeatureAvailability {
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self { available: false, reason: Some(reason.into()) }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeStatus {
    pub generation: u64,
    pub services_healthy: usize,
    pub services_degraded: usize,
    pub services_unavailable: usize,
    pub route_health: Option<String>,
    pub active_jobs: Option<usize>,
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
            UiFeature::InlineImage => "inline-image",
            UiFeature::Steering => "steer-active-turn",
        };
        let snapshot = capability::for_cwd(cwd);
        let mut reasons = Vec::new();
        let mut described = false;
        for descriptor in snapshot.descriptors.iter().filter(|descriptor| descriptor.operations.iter().any(|candidate| candidate == operation)) {
            described = true;
            if descriptor.ui.executable && descriptor.health.is_executable() {
                return FeatureAvailability { available: true, reason: None };
            }
            if let Some(detail) = &descriptor.health.detail { reasons.push(detail.clone()); }
        }
        #[cfg(target_os = "macos")]
        if !described && matches!(feature, UiFeature::ClipboardRead | UiFeature::ClipboardImage) {
            return FeatureAvailability { available: true, reason: None };
        }
        let reason = if reasons.is_empty() {
            format!("No executable capability provides operation `{operation}`")
        } else {
            reasons.join("; ")
        };
        FeatureAvailability::unavailable(reason)
    }

    fn read_clipboard(&self, cwd: &Path) -> Result<Option<ClipboardContent>, String> {
        let image = self.availability(cwd, UiFeature::ClipboardImage);
        let text = self.availability(cwd, UiFeature::ClipboardRead);
        #[cfg(target_os = "macos")]
        {
            let mut image_error = None;
            if image.available {
                let path = std::env::temp_dir().join(format!("jeden-clipboard-{}-{}.png", std::process::id(), capability::snapshot().generation));
                let escaped = path.display().to_string().replace('\\', "\\\\").replace('"', "\\\"");
                let script = format!("set imageFile to open for access POSIX file \"{escaped}\" with write permission\nset eof imageFile to 0\nwrite (the clipboard as «class PNGf») to imageFile\nclose access imageFile");
                match std::process::Command::new("osascript").args(["-e", &script]).output() {
                    Ok(output) if output.status.success() => {
                        let bytes = std::fs::read(&path).map_err(|error| format!("Clipboard image read failed: {error}"));
                        let _ = std::fs::remove_file(&path);
                        return bytes.map(|bytes| Some(ClipboardContent::Bytes { name: "clipboard.png".into(), bytes: Arc::from(bytes) }));
                    }
                    Ok(output) => image_error = Some(String::from_utf8_lossy(&output.stderr).trim().to_string()),
                    Err(error) => image_error = Some(error.to_string()),
                }
                let _ = std::fs::remove_file(&path);
            }
            if text.available {
                let output = std::process::Command::new("pbpaste").output().map_err(|error| format!("Clipboard text read failed: {error}"))?;
                if !output.status.success() { return Err(String::from_utf8_lossy(&output.stderr).trim().to_string()); }
                if output.stdout.is_empty() { return Ok(None); }
                return String::from_utf8(output.stdout).map(|value| Some(ClipboardContent::Text(value))).map_err(|error| format!("Clipboard text is not UTF-8: {error}"));
            }
            return Err(image_error.or(image.reason).or(text.reason).unwrap_or_else(|| "Clipboard unavailable".into()));
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (image, text);
            Err("No native clipboard adapter is available on this platform".into())
        }
    }

    fn runtime_status(&self, cwd: &Path) -> RuntimeStatus {
        let snapshot = capability::for_cwd(cwd);
        let mut status = RuntimeStatus { generation: snapshot.generation, ..RuntimeStatus::default() };
        for descriptor in snapshot.kind(CapabilityKind::Service) {
            match descriptor.health.state {
                HealthState::Healthy => status.services_healthy += 1,
                HealthState::Degraded => status.services_degraded += 1,
                HealthState::Unavailable | HealthState::Disabled => status.services_unavailable += 1,
            }
            if descriptor.operations.iter().any(|operation| operation == "route-health") {
                status.route_health = Some(match descriptor.health.state {
                    HealthState::Healthy => "healthy",
                    HealthState::Degraded => "degraded",
                    HealthState::Unavailable => "unavailable",
                    HealthState::Disabled => "disabled",
                }.to_string());
            }
            if let Some(running) = descriptor.metadata.get("runningJobs").and_then(|value| value.as_u64()) {
                status.active_jobs = usize::try_from(running).ok();
            }
        }
        status
    }
}
