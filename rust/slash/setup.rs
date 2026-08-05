//! Guided environment configuration wizard (`/setup`).
//!
//! The wizard is idempotent: every step checks live state first and renders
//! already-configured items as disabled `[OK]` rows. Nonsecret router settings
//! may be persisted locally; credentials are injected by the trusted Stado
//! launcher and are never written by the wizard.

use std::fs;
use std::path::{Path, PathBuf};

use super::common::{dirs_home, split_head};
use super::SlashContext;
use crate::cli::config::{load_config, ui_language};
use crate::tui::{CommandOutcome, PickerItem, PickerSpec};

const BRAMA_URL_KEY: &str = "BRAMA_URL";
const AGENT_ID_KEY: &str = "WISENT_APP_AGENT_ID";
const AGENT_SECRET_KEY: &str = "WISENT_APP_AGENT_AUTH_SECRET";
const DEFAULT_AGENT_ID: &str = "wisent-app";

fn env_file_path() -> PathBuf {
    dirs_home().join(".jeden/.env")
}

/// Parse one `KEY=value` line the same way the main env loader does: trim,
/// strip a trailing ` #` comment, unquote, and expand `\n`.
fn parse_env_line_value(raw: &str) -> String {
    let mut value = raw.trim().to_string();
    if let Some(index) = value.find(" #") {
        value.truncate(index);
        value = value.trim().to_string();
    }
    let unquoted = value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')));
    if let Some(inner) = unquoted {
        value = inner.to_string();
    }
    value.replace("\\n", "\n")
}

fn env_file_value(path: &Path, key: &str) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((name, raw_value)) = trimmed.split_once('=') else {
            continue;
        };
        if name.trim() == key {
            let value = parse_env_line_value(raw_value);
            if !value.trim().is_empty() {
                return Some(value);
            }
        }
    }
    None
}

/// Effective value for a router key: the process environment wins, then the
/// persisted `~/.jeden/.env` file. Empty values count as unconfigured.
fn configured_value(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| env_file_value(&env_file_path(), key))
}

/// True when the required Brama model-router endpoint is available
/// from the process environment or `~/.jeden/.env`. Drives the welcome tip.
pub(crate) fn brama_router_configured(_cwd: &Path) -> bool {
    configured_value(BRAMA_URL_KEY).is_some()
}

/// Prefill for an INPUT row, read best-effort from the repository's
/// `.env.example` (cwd and up to two parents, so subdirectories work too).
fn example_prefill(cwd: &Path, key: &str) -> Option<String> {
    let mut dir = Some(cwd);
    for _ in 0..3 {
        let candidate = dir?.join(".env.example");
        if let Some(value) = env_file_value(&candidate, key) {
            return Some(value);
        }
        dir = dir?.parent();
    }
    None
}

/// Append or update `updates` in `~/.jeden/.env`, preserving every unrelated
/// line byte-for-byte, and force the file to mode 0600. Values that would not
/// survive the loader's comment/quote parsing are written double-quoted.
fn write_env_keys(updates: &[(String, String)]) -> Result<PathBuf, String> {
    let path = env_file_path();
    let encode = |key: &str, value: &str| {
        if value.contains('#') || value.contains(char::is_whitespace) {
            format!("{key}=\"{value}\"")
        } else {
            format!("{key}={value}")
        }
    };
    let existing = fs::read_to_string(&path).unwrap_or_default();
    let mut pending: Vec<&(String, String)> = updates.iter().collect();
    let mut lines: Vec<String> = existing.lines().map(str::to_string).collect();
    for line in lines.iter_mut() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((name, _)) = trimmed.split_once('=') else {
            continue;
        };
        let name = name.trim();
        if let Some(position) = pending.iter().position(|(key, _)| key == name) {
            let (key, value) = pending.remove(position);
            *line = encode(key, value);
        }
    }
    for (key, value) in pending {
        lines.push(encode(key, value));
    }
    let mut text = lines.join("\n");
    text.push('\n');
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs::write(&path, text).map_err(|error| error.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .map_err(|error| error.to_string())?;
    }
    // Make the values effective for this running session (catalog fetch,
    // agent identity) without requiring a restart.
    for (key, value) in updates {
        std::env::set_var(key, value);
    }
    Ok(path)
}

fn save_brama_url(value: &str) -> Result<String, String> {
    let value = value.trim();
    if !(value.starts_with("https://") || value.starts_with("http://")) {
        return Err("BRAMA_URL must start with https:// or http://".into());
    }
    let path = write_env_keys(&[(BRAMA_URL_KEY.into(), value.into())])?;
    Ok(format!(
        "Saved BRAMA_URL to {} (0600).",
        path.display()
    ))
}

fn save_agent_id(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() || value.contains(char::is_whitespace) {
        return Err("WISENT_APP_AGENT_ID must be a single non-empty token".into());
    }
    let path = write_env_keys(&[(AGENT_ID_KEY.into(), value.into())])?;
    Ok(format!(
        "Saved WISENT_APP_AGENT_ID ({value}) to {} (0600).",
        path.display()
    ))
}


struct SetupState {
    brama_url: Option<String>,
    agent_id: Option<String>,
    secret_configured: bool,
    model: Option<String>,
    language: String,
    theme: String,
}

fn setup_state(context: &SlashContext<'_>) -> SetupState {
    let config = load_config(context.cwd);
    let model = context
        .model
        .map(str::to_string)
        .or(config.model.clone())
        .or_else(|| std::env::var("JEDEN_MODEL").ok())
        .filter(|value| !value.trim().is_empty());
    let theme = crate::cli::config::merged_config_value(context.cwd)
        .get("ui")
        .and_then(|ui| ui.get("theme"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("auto")
        .to_string();
    SetupState {
        brama_url: configured_value(BRAMA_URL_KEY),
        agent_id: configured_value(AGENT_ID_KEY),
        secret_configured: configured_value(AGENT_SECRET_KEY).is_some(),
        model,
        language: ui_language(&config).code().to_string(),
        theme,
    }
}

fn configured_row(label: impl Into<String>, detail: impl Into<String>) -> PickerItem {
    PickerItem::action(label, "")
        .detail(detail)
        .badge("OK")
        .disabled(true)
}

/// Wizard overview picker: one row per step, `[OK]`-disabled when already
/// configured, INPUT/picker dispatch when action is needed.
pub(crate) fn setup_picker(context: &SlashContext<'_>) -> Result<PickerSpec, String> {
    let lang = crate::cli::i18n::lang_code(context.cwd);
    let state = setup_state(context);
    let mut items = Vec::new();
    match &state.brama_url {
        Some(url) => items.push(configured_row(
            "1. BRAMA_URL configured",
            format!("{url} · stored in ~/.jeden/.env"),
        )),
        None => {
            let prefill = example_prefill(context.cwd, BRAMA_URL_KEY)
                .map(|url| format!(" {url}"))
                .unwrap_or_else(|| " ".into());
            items.push(
                PickerItem::action(
                    "1. Set BRAMA_URL",
                    format!("/setup brama-url{prefill}"),
                )
                .detail("required Brama model-router endpoint · stored in ~/.jeden/.env")
                .badge("INPUT")
                .prefill(),
            );
        }
    }
    match &state.agent_id {
        Some(id) => items.push(configured_row(
            "2. WISENT_APP_AGENT_ID configured",
            format!("{id} · stored in ~/.jeden/.env"),
        )),
        None => items.push(
            PickerItem::action(
                "2. Set WISENT_APP_AGENT_ID",
                format!("/setup agent-id {DEFAULT_AGENT_ID}"),
            )
            .detail(format!("default: {DEFAULT_AGENT_ID} · stored in ~/.jeden/.env"))
            .badge("INPUT")
            .prefill(),
        ),
    }
    if state.secret_configured {
        items.push(configured_row(
            "3. WISENT_APP_AGENT_AUTH_SECRET configured",
            "injected in memory by the Stado/Skarbiec launcher",
        ));
    } else {
        items.push(configured_row(
            "3. WISENT_APP_AGENT_AUTH_SECRET unavailable",
            "launch with bin/jeden-rust or scripts/run-with-stado.sh",
        ));
    }
    match &state.model {
        Some(model) => items.push(configured_row(
            format!("4. Model route: {model}"),
            "change anytime with /model",
        )),
        None => items.push(
            PickerItem::action("4. Select model route", "/setup model")
                .detail("fetch the Brama catalog and pick a route")
                .badge("MODEL"),
        ),
    }
    items.push(
        PickerItem::action("5. Language & theme (optional)", "/setup preferences")
            .detail(format!(
                "current: language {} · theme {}",
                state.language, state.theme
            ))
            .badge("PREFS"),
    );
    items.push(
        PickerItem::action("6. Validate setup", "/setup validate")
            .detail("run doctor and show the final summary")
            .badge("CHECK"),
    );
    Ok(PickerSpec::new("Setup — first-run configuration", items).localized(&lang))
}

/// Optional preferences: the existing enum setting rows from the settings
/// view, filtered to `ui.language` and `ui.theme` — selection dispatches the
/// stock `/settings set ...` commands, nothing is rebuilt.
fn preferences_picker(cwd: &Path) -> PickerSpec {
    let lang = crate::cli::i18n::lang_code(cwd);
    let settings = crate::cli::config::schema::settings_picker(cwd);
    let items = settings
        .items
        .into_iter()
        .filter(|item| {
            item.command.as_deref().is_some_and(|command| {
                command.starts_with("/settings set ui.language ")
                    || command.starts_with("/settings set ui.theme ")
            })
        })
        .collect();
    PickerSpec::new("Preferences — language & theme", items).localized(&lang)
}

fn checklist_text(context: &SlashContext<'_>) -> String {
    let state = setup_state(context);
    let mark = |configured: bool| if configured { "[OK]" } else { "[missing]" };
    let mut lines = vec![
        "Setup checklist (guided wizard: run /setup in the interactive TUI):".to_string(),
        format!(
            "1. BRAMA_URL {} — required; set: jeden run \"/setup brama-url <https-url>\" (or: echo 'BRAMA_URL=<url>' >> ~/.jeden/.env)",
            mark(state.brama_url.is_some())
        ),
        format!(
            "2. WISENT_APP_AGENT_ID {} — set: jeden run \"/setup agent-id <id>\" (default: {DEFAULT_AGENT_ID})",
            mark(state.agent_id.is_some())
        ),
        format!(
            "3. WISENT_APP_AGENT_AUTH_SECRET {} — Skarbiec item agent:wisent-app/value via scripts/run-with-stado.sh",
            mark(state.secret_configured)
        ),
        "   Only nonsecret router settings are written to ~/.jeden/.env; credentials are injected in memory by the trusted Stado launcher."
            .to_string(),
        match &state.model {
            Some(model) => format!("4. Model route [OK] {model} — change with: /model <route>"),
            None => "4. Model route [missing] — pick in the TUI with /model (writes .jeden/config.json)".to_string(),
        },
        format!(
            "5. Preferences (optional) [language {} · theme {}] — jeden config set ui.language <code> · jeden config set ui.theme <name>",
            state.language, state.theme
        ),
        "6. Validate — jeden doctor · smoke: jeden run \"Respond exactly: OK\"".to_string(),
    ];
    lines.push("Status is read-only: nothing was changed.".to_string());
    lines.join("\n")
}

fn validate_text(context: &SlashContext<'_>) -> String {
    let report = crate::conformance::health::doctor(context.cwd);
    let probe_line = |name: &str| -> String {
        match report.probes.iter().find(|probe| probe.subsystem == name) {
            Some(probe) => format!(
                "- {name}: {} — {}",
                format!("{:?}", probe.state).to_ascii_lowercase(),
                probe.detail
            ),
            None => format!("- {name}: not probed"),
        }
    };
    let state = setup_state(context);
    let available = report
        .probes
        .iter()
        .filter(|probe| probe.state != crate::conformance::health::ProbeState::Unavailable)
        .count();
    let mut lines = vec![
        "Setup validation (jeden doctor):".to_string(),
        probe_line("brama"),
        probe_line("weles"),
        format!(
            "- model: {}",
            state.model.unwrap_or_else(|| "not selected — pick one with /model".into())
        ),
        format!("- language: {}", state.language),
        format!(
            "Probes available: {}/{} · overall: {}",
            available,
            report.probes.len(),
            if report.healthy { "healthy" } else { "unhealthy" }
        ),
        "Smoke: try: jeden run \"Respond exactly: OK\"".to_string(),
    ];
    if !report.healthy {
        lines.push("Fix the failing steps above, then rerun /setup validate.".to_string());
    }
    lines.join("\n")
}

const USAGE: &str = "Usage: /setup [status|validate|brama-url <url>|agent-id <id>|model|preferences]";

/// Text-mode handler (non-interactive callers such as `jeden run "/setup …"`
/// and the slash fallback). Bare `/setup` and `/setup status` print the
/// checklist without changing anything.
pub(crate) fn handle_text(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let (verb, rest) = split_head(args);
    match verb.to_ascii_lowercase().as_str() {
        "" | "status" => Ok(checklist_text(context)),
        "validate" => Ok(validate_text(context)),
        "brama-url" => save_brama_url(rest),
        "agent-id" => save_agent_id(rest),
        "model" | "preferences" => Ok(format!(
            "That step needs the interactive TUI; run bare `jeden` and then /setup.\n\n{}",
            checklist_text(context)
        )),
        _ => Err(USAGE.into()),
    }
}

/// Interactive handler for the TUI: step submissions keep the wizard sequence
/// alive by returning the next view instead of plain text.
pub(crate) fn run_interactive(
    cwd: &Path,
    args: &str,
    current_model: Option<&str>,
    interactive: bool,
) -> Result<CommandOutcome, String> {
    let session_root = crate::session_root();
    let context = SlashContext {
        cwd,
        model: current_model,
        session_root: &session_root,
    };
    let (verb, rest) = split_head(args);
    match verb.to_ascii_lowercase().as_str() {
        "" if interactive => setup_picker(&context).map(CommandOutcome::Picker),
        "" | "status" => Ok(CommandOutcome::Text(checklist_text(&context))),
        "validate" => Ok(CommandOutcome::Text(validate_text(&context))),
        "brama-url" | "agent-id" => {
            let message = match verb.to_ascii_lowercase().as_str() {
                "brama-url" => save_brama_url(rest)?,
                _ => save_agent_id(rest)?,
            };
            if interactive {
                // Re-open the wizard so the user lands on the next missing step.
                setup_picker(&context).map(CommandOutcome::Picker)
            } else {
                Ok(CommandOutcome::Text(message))
            }
        }
        "model" => crate::cli::run::slash_ui::model_picker(cwd, current_model, false)
            .map(CommandOutcome::Picker)
            .map_err(|error| {
                format!(
                    "{error}\nHint: fix the Brama connection (step 1 writes ~/.jeden/.env), then rerun /setup. Completed steps are kept."
                )
            }),
        "preferences" => Ok(CommandOutcome::Picker(preferences_picker(cwd))),
        _ => Err(USAGE.into()),
    }
}
