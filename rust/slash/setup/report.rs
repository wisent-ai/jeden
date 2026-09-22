//! Telling the operator in plain text what is configured and what the machine
//! actually answered when it was asked.
//!
//! Split out of `slash/setup.rs`, which had grown past the module line cap.

use super::super::SlashContext;
use super::setup_state;

pub(super) fn checklist_text(context: &SlashContext<'_>) -> String {
    let state = setup_state(context);
    let mark = |configured: bool| if configured { "[OK]" } else { "[missing]" };
    let workspace_line = match (&state.workspace, &state.workspace_error) {
        (Some(path), _) => format!(
            "1. Workspace [OK] {} — existing files and matching Jeden sessions remain in place",
            path.display()
        ),
        (None, Some(error)) => format!("1. Workspace [invalid] — {error}"),
        (None, None) => format!(
            "1. Workspace [missing] — adopt: jeden workspace adopt {}",
            context.cwd.display()
        ),
    };
    let mut lines = vec![
        "Setup checklist (guided wizard: run /setup in the interactive TUI):".to_string(),
        workspace_line,
        format!(
            "2. BRAMA_URL {} — required; set: jeden run \"/setup brama-url <https-url>\" (or: echo 'BRAMA_URL=<url>' >> ~/.jeden/.env)",
            mark(state.brama_url.is_some())
        ),
        format!(
            "3. WISENT_APP_AGENT_ID {} — set: jeden run \"/setup agent-id <id>\" (default: {DEFAULT_AGENT_ID})",
            mark(state.agent_id.is_some())
        ),
        format!(
            "4. WISENT_APP_AGENT_AUTH_SECRET {} — Skarbiec item agent:wisent-app/value via scripts/run-with-stado.sh",
            mark(state.secret_configured)
        ),
        "   Only nonsecret router settings are written to ~/.jeden/.env; credentials are injected in memory by the trusted Stado launcher."
            .to_string(),
        match &state.model {
            Some(model) => format!("5. Model route [OK] {model} — change with: /model <route>"),
            None => "5. Model route [missing] — pick in the TUI with /model (writes .jeden/config.json)".to_string(),
        },
        format!(
            "6. Preferences (optional) [language {} · theme {}] — jeden config set ui.language <code> · jeden config set ui.theme <name>",
            state.language, state.theme
        ),
        "7. Validate — /setup validate".to_string(),
    ];
    lines.push("Status is read-only: nothing was changed.".to_string());
    lines.join("\n")
}

pub(super) fn validate_text(context: &SlashContext<'_>) -> String {
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
        match (&state.workspace, &state.workspace_error) {
            (Some(path), _) => format!("- workspace: adopted — {}", path.display()),
            (None, Some(error)) => format!("- workspace: invalid — {error}"),
            (None, None) => "- workspace: not adopted — run /setup workspace <path>".into(),
        },
        probe_line("brama"),
        probe_line("weles"),
        format!(
            "- model: {}",
            state
                .model
                .unwrap_or_else(|| "not selected — pick one with /model".into())
        ),
        format!("- language: {}", state.language),
        format!(
            "Probes available: {}/{} · overall: {}",
            available,
            report.probes.len(),
            if report.healthy {
                "healthy"
            } else {
                "unhealthy"
            }
        ),
        "Smoke: try: jeden run \"Respond exactly: OK\"".to_string(),
    ];
    if !report.healthy {
        lines.push("Fix the failing steps above, then rerun /setup validate.".to_string());
    }
    lines.join("\n")
}
