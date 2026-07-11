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
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

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
    cwd: PathBuf,
    sessions: Mutex<BTreeMap<String, String>>,
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
            cwd: cwd.to_path_buf(),
            sessions: Mutex::new(BTreeMap::new()),
            next_session: AtomicU64::new(1),
        }
    }
    pub(crate) fn health(&self) -> HealthDescriptor {
        match self.bridge.as_deref() {
            Some(bridge) if command_exists(bridge) => HealthDescriptor::healthy("browser", bridge),
            Some(bridge) => HealthDescriptor::unavailable("browser", format!("configured bridge {bridge} is not executable")),
            None => HealthDescriptor::unavailable("browser", "set JEDEN_BROWSER_BRIDGE or toolServices.browser.bridge to a Chromium/CDP JSON bridge"),
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
        let bridge = self.bridge.as_deref().expect("available browser bridge");
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
        let request = json!({ "session": session, "action": action, "input": input });
        let response = process::run_json(
            "browser",
            context,
            &self.cwd,
            bridge,
            &["--session".into(), session.clone()],
            Some(
                serde_json::to_vec(&request).map_err(|e| ServiceError::Protocol {
                    service: "browser",
                    detail: e.to_string(),
                })?,
            ),
            Duration::from_secs(60),
        )?;
        if response.get("ok").and_then(Value::as_bool) == Some(false) {
            return Err(ServiceError::Backend {
                service: "browser",
                detail: response
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("bridge rejected request")
                    .into(),
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
            let mut artifact = write_media_artifact(
                context,
                "browser-screenshot",
                response
                    .get("format")
                    .and_then(Value::as_str)
                    .unwrap_or("png"),
                &bytes,
            )?;
            artifact["session"] = json!(session);
            artifact["tab"] = response.get("tab").cloned().unwrap_or(Value::Null);
            return Ok(artifact);
        }
        bounded_json(context, "browser", &response)
    }

    #[cfg(test)]
    pub(crate) fn session_id_for_test(&self, key: &str) -> String {
        self.session(&json!({"session": key}))
    }
}
