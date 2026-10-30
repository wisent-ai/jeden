//! Telling the operator in plain text what is configured and what the machine
//! actually answered when it was asked.
//!
//! Split out of `slash/setup.rs`, which had grown past the module line cap.

use super::super::SlashContext;
use super::setup_state;

pub(super) fn checklist_text(context: &SlashContext<'_>) -> String {
    let state = setup_state(context);
    let mark = |configured: bool| {
        if configured {
            "[OK]"
        } else if state.env_file_error.is_some() {
            "[unknown]"
        } else {
            "[missing]"
        }
    };
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
    if let Some((endpoint, key)) = crate::agent::credential::direct_provider() {
        let credential = if key.is_some() {
            match crate::agent::credential::standing_in() {
                Some("stado-resolved") => "Provider key: the key of the role Stado's capability resolution names for the endpoint answering in Brama's place".to_string(),
                Some(_) => "Provider key: the bearer of Jeden's own gateway (signed in with jeden token sign-in)".to_string(),
                None => "Provider key: JEDEN_MODEL_KEY from the process environment".to_string(),
            }
        } else {
            match crate::agent::credential::tokens::Store::open_existing() {
                Ok(Some(store)) => match store.count_for(&endpoint) {
                    Ok(count) => format!("Provider keys: {count} stored for this endpoint"),
                    Err(error) => format!("Provider keys could not be counted: {error}"),
                },
                Ok(None) => "No key configured for this endpoint; anonymous providers may work. If authentication is required, add with `jeden token add <label> --endpoint <url>` or set JEDEN_MODEL_KEY".to_string(),
                Err(error) => format!("Provider token store could not be opened: {error}"),
            }
        };
        let model = match &state.model {
            Some(model) => format!("Model route: {model}"),
            None => "Model route not selected — choose with /model".to_string(),
        };
        let endpoint_line = "Endpoint: configured by JEDEN_MODEL_ENDPOINT".to_string();
        let mut lines = vec![
            "Setup checklist (direct provider):".to_string(),
            workspace_line,
            endpoint_line,
            credential,
            model,
            format!(
                "Preferences: language {} · theme {}",
                state.language, state.theme
            ),
            "Validate with /setup validate; status is read-only.".to_string(),
        ];
        if let Some(error) = &state.model_error {
            lines.push(error.clone());
        }
        return lines.join("\n");
    }
    if let Some(cause) = crate::agent::credential::direct_refusal() {
        return format!("Setup checklist (direct provider):\n{workspace_line}\nEndpoint: refused — {cause}\nValidate with /setup validate; status is read-only.");
    }
    let mut lines = vec![
        "Setup checklist (guided wizard: run /setup in the interactive TUI):".to_string(),
        workspace_line,
        if state.brama_url.is_none() && state.env_file_error.is_some() {
            "2. BRAMA_URL [unknown] — resolve the environment file read error before configuring this value".to_string()
        } else {
            format!(
                "2. BRAMA_URL {} — {}; on a Stado host the route comes from `stado capabilities resolve inference --consumer jeden`, elsewhere set: jeden run \"/setup brama-url <https-url>\"",
                mark(state.brama_url.is_some()),
                crate::agent::credential::route_origin()
            )
        },
        if state.agent_id.is_none() && state.env_file_error.is_some() {
            "3. WISENT_APP_AGENT_ID [unknown] — resolve the environment file read error before configuring this value".to_string()
        } else {
            format!(
                "3. WISENT_APP_AGENT_ID {} — set: jeden run \"/setup agent-id <id>\", the agent Brama grants the signing secret to",
                mark(state.agent_id.is_some())
            )
        },
        format!(
            "4. WISENT_APP_AGENT_AUTH_SECRET {} — supplied by the process environment to sign Brama requests",
            mark(state.secret_configured)
        ),
        "   BRAMA_TOKEN is supplied by the process environment for Brama requests; no credential is fetched from Skarbiec."
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
    if let Some(error) = &state.env_file_error {
        lines.push(format!(
            "~/.jeden/.env [unreadable] — {error}; the values it holds are unknown, not missing"
        ));
    }
    if let Some(error) = &state.model_error {
        lines.push(error.clone());
    }
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
        probe_line(
            if crate::agent::credential::direct_provider().is_some()
                || crate::agent::credential::direct_refusal().is_some()
            {
                "model"
            } else {
                "brama"
            },
        ),
        probe_line("weles"),
        format!(
            "- selected model: {}",
            match &state.model {
                Some(model) => model.as_str(),
                None => "not selected — pick one with /model",
            }
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
