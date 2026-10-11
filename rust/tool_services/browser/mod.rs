use super::config;
use super::process;
use super::types::{
    bounded_json, command_exists, nonempty, write_media_artifact, HealthDescriptor, ServiceError,
    ServiceResult,
};
use crate::tool_runtime::runtime_ops::OperationContext;
use base64::Engine;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(unix)]
unsafe extern "C" {
    fn kill(pid: i32, signal: i32) -> i32;
}

// The bridge is one node script kept in two files; they run as one program.
const EMBEDDED_BRIDGE: &str = concat!(
    include_str!("bridge-core.mjs"),
    include_str!("bridge-actions.mjs")
);

mod mode;
use mode::{browser_profile, browser_visible, embedded_backend_health};
pub(crate) use mode::{HEADLESS, VISIBLE};

fn default_chromium(config: &Value) -> Option<String> {
    config::string(
        config,
        &["toolServices", "browser", "chrome"],
        "JEDEN_CHROME_EXECUTABLE",
    )
    .or_else(|| {
        config::string(
            config,
            &["browser", "launch", "executablePath"],
            "JEDEN_CHROME_EXECUTABLE",
        )
    })
    .or_else(|| {
        [
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
            "google-chrome",
            "chromium",
            "chromium-browser",
        ]
        .into_iter()
        .find(|candidate| command_exists(candidate))
        .map(str::to_owned)
    })
}

pub(crate) const TOOLS: &[(&str, &str)] = &[
    (
        "browser_tab",
        "Open, list, focus, or close a reusable Chromium/CDP tab",
    ),
    (
        "browser_action",
        "Navigate, click, type, evaluate, wait, scroll, or inspect a browser tab",
    ),
    (
        "browser_screenshot",
        "Capture a browser tab screenshot as a session artifact",
    ),
];

pub(crate) struct BrowserService {
    bridge: Option<String>,
    chrome: Option<String>,
    visible: Result<bool, String>,
    profile: Option<String>,
    cwd: PathBuf,
    sessions: Mutex<BTreeMap<String, String>>,
    browser_pids: Mutex<BTreeSet<u32>>,
    next_session: AtomicU64,
}

impl BrowserService {
    pub(crate) fn discover(cwd: &Path, value: &Value) -> Self {
        Self {
            bridge: config::string(
                value,
                &["toolServices", "browser", "bridge"],
                "JEDEN_BROWSER_BRIDGE",
            ),
            chrome: default_chromium(value),
            visible: browser_visible(value),
            profile: browser_profile(value),
            cwd: cwd.to_path_buf(),
            sessions: Mutex::new(BTreeMap::new()),
            browser_pids: Mutex::new(BTreeSet::new()),
            next_session: AtomicU64::new(1),
        }
    }
    pub(crate) fn health(&self) -> HealthDescriptor {
        if let Some(bridge) = self.bridge.as_deref() {
            return if command_exists(bridge) {
                HealthDescriptor::healthy("browser", bridge)
            } else {
                HealthDescriptor::unavailable(
                    "browser",
                    format!("configured bridge {bridge} is not executable"),
                )
            };
        }
        let visible = match &self.visible {
            Ok(visible) => *visible,
            Err(error) => return HealthDescriptor::unavailable("browser", error.clone()),
        };
        match embedded_backend_health(self.chrome.as_deref()) {
            Ok(chrome) => HealthDescriptor::healthy(
                "browser",
                format!(
                    "embedded-node-cdp ({chrome}, {})",
                    if visible { VISIBLE } else { HEADLESS }
                ),
            ),
            Err(error) => HealthDescriptor::unavailable("browser", error),
        }
    }
    fn session(&self, input: &Value) -> String {
        let key = input
            .get("session")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
            .unwrap_or("default");
        let mut sessions = self.sessions.lock();
        sessions
            .entry(key.to_string())
            .or_insert_with(|| {
                format!(
                    "jeden-browser-{}-{}",
                    std::process::id(),
                    self.next_session.fetch_add(1, Ordering::Relaxed)
                )
            })
            .clone()
    }
    pub(crate) fn execute(
        &self,
        tool: &str,
        input: &Value,
        context: &OperationContext<'_>,
    ) -> ServiceResult<Value> {
        let health = self.health();
        if !health.available() {
            return Err(ServiceError::Unavailable {
                service: "browser",
                detail: health.detail,
            });
        }
        let action = match tool {
            "browser_tab" => nonempty(input.get("action"), "action")?,
            "browser_action" => nonempty(input.get("action"), "action")?,
            "browser_screenshot" => "screenshot".into(),
            _ => {
                return Err(ServiceError::InvalidInput(format!(
                    "unknown browser tool {tool}"
                )))
            }
        };
        let session = self.session(input);
        let request = json!({ "session": session, "tool": tool, "action": action, "input": input });
        let mut args = vec!["--session".into(), session.clone()];
        let program = if let Some(bridge) = self.bridge.as_deref() {
            bridge
        } else {
            args.splice(
                0..0,
                [
                    "--input-type=module".into(),
                    "--eval".into(),
                    EMBEDDED_BRIDGE.into(),
                    "--".into(),
                ],
            );
            args.push("--chrome".into());
            args.push(
                self.chrome
                    .as_ref()
                    .expect("healthy embedded browser backend")
                    .clone(),
            );
            if self.visible == Ok(true) {
                args.push("--visible".into());
            }
            if let Some(profile) = &self.profile {
                args.push("--user-data-dir".into());
                args.push(profile.clone());
            }
            "node"
        };
        let response = process::run_json(
            "browser",
            context,
            &self.cwd,
            program,
            &args,
            Some(
                serde_json::to_vec(&request).map_err(|e| ServiceError::Protocol {
                    service: "browser",
                    detail: e.to_string(),
                })?,
            ),
        )?;
        if self.bridge.is_none() {
            if let Some(pid) = response.get("browserPid").and_then(Value::as_u64) {
                if let Ok(pid) = u32::try_from(pid) {
                    self.browser_pids.lock().insert(pid);
                }
            }
        }
        if response.get("ok").and_then(Value::as_bool) == Some(false) {
            let detail = response
                .get("error")
                .and_then(Value::as_str)
                .ok_or_else(|| ServiceError::Protocol {
                    service: "browser",
                    detail: format!(
                        "the bridge answered ok:false with no error string: {response}"
                    ),
                })?;
            return Err(ServiceError::Backend {
                service: "browser",
                detail: detail.into(),
            });
        }
        if tool == "browser_screenshot" {
            let encoded = response
                .get("data")
                .and_then(Value::as_str)
                .ok_or_else(|| ServiceError::Protocol {
                    service: "browser",
                    detail: "screenshot response lacks base64 data".into(),
                })?;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .map_err(|e| ServiceError::Protocol {
                    service: "browser",
                    detail: e.to_string(),
                })?;
            let format = response
                .get("format")
                .and_then(Value::as_str)
                .ok_or_else(|| ServiceError::Protocol {
                    service: "browser",
                    detail: format!("screenshot response names no image format: {response}"),
                })?;
            let mut artifact = write_media_artifact(context, "browser-screenshot", format, &bytes)?;
            artifact["session"] = json!(session);
            artifact["tab"] =
                response
                    .get("tab")
                    .cloned()
                    .ok_or_else(|| ServiceError::Protocol {
                        service: "browser",
                        detail: format!("screenshot response names no tab: {response}"),
                    })?;
            return Ok(artifact);
        }
        bounded_json(context, "browser", &response)
    }
}

impl Drop for BrowserService {
    fn drop(&mut self) {
        #[cfg(unix)]
        for pid in self.browser_pids.get_mut().iter().copied() {
            unsafe {
                let _ = kill(pid as i32, 9);
            }
        }
    }
}
