//! `jeden token` — which Brama credential this agent uses: the router URL,
//! the agent id, where the secret came from and a redacted form of it.
//! Provider OAuth tokens live in Skarbiec/Brama and are never held by jeden.
//! The secret itself is never printed (cli.md rule 15): outside the
//! credential boundary only its Skarbiec item, `agent:wisent-app`, travels,
//! and a script that needs the value reads that item through its own grant.

use std::env;

use crate::Args;

const SECRET_KEY: &str = "WISENT_APP_AGENT_AUTH_SECRET";
const AGENT_ID_KEY: &str = "WISENT_APP_AGENT_ID";

fn redacted(value: &str) -> String {
    let tail: String = value
        .chars()
        .rev()
        .take(4)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    format!("…{tail} ({} chars)", value.chars().count())
}

fn brama_url() -> Result<String, String> {
    env::var("BRAMA_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            "BRAMA_URL is required; configure the Brama model-router service URL".to_string()
        })
}

fn configured() -> Result<(String, String, String, &'static str), String> {
    // Where the value came from is part of the answer: an operator reading
    // this needs to know whether the launcher carried it or the vault did.
    // The same resolution every request path uses: the secret comes from
    // Stado when this process was not launched carrying it. The refusal below
    // used to name `scripts/run-with-stado.sh`, a file this repository
    // deleted, which left a reader with an instruction that could not be
    // followed; it now carries what Stado itself said.
    let (secret_source, _) = crate::agent::credential::ensure();
    let url = brama_url()?;
    let secret = env::var(SECRET_KEY).unwrap_or_default();
    if secret.is_empty() {
        return Err(match secret_source.refusal() {
            Some(said) => format!("{SECRET_KEY} is not configured: {said}"),
            None => format!(
                "{SECRET_KEY} is not configured; Skarbiec item `agent:wisent-app` holds it and \
                 `stado credentials get agent:wisent-app --field value` is how this process reads it"
            ),
        });
    }
    Ok((
        url,
        env::var(AGENT_ID_KEY).unwrap_or_default(),
        secret,
        secret_source.as_str(),
    ))
}

/// CLI `jeden token [--list] [--json]`.
pub(crate) fn token_command(args: &Args) -> Result<String, String> {
    if let Some(unknown) = args.positionals.iter().find(|part| *part != "--list" && *part != "list") {
        return Err(format!(
            "jeden token takes --list and --json; got {unknown}. The secret is never printed: read the Skarbiec item agent:wisent-app through a grant of its own"
        ));
    }
    let list = !args.positionals.is_empty();
    let (brama, agent_id, secret, source) = configured()?;
    if args.json {
        return Ok(format!(
            "{{\"bramaUrl\":{},\"agentId\":{},\"token\":{},\"tokenSource\":{},\"tokenItem\":\"agent:wisent-app\"}}\n",
            serde_json::to_string(&brama).map_err(|error| error.to_string())?,
            serde_json::to_string(&agent_id).map_err(|error| error.to_string())?,
            serde_json::to_string(&redacted(&secret)).map_err(|error| error.to_string())?,
            serde_json::to_string(source).map_err(|error| error.to_string())?,
        ));
    }
    let mut lines = vec![
        format!("Brama:   {brama}"),
        format!("Agent:   {agent_id}"),
        match source {
            "stado" => format!(
                "Token:   {} — read through Stado from the Skarbiec item agent:wisent-app",
                redacted(&secret)
            ),
            _ => format!(
                "Token:   {} — carried in this process's environment",
                redacted(&secret)
            ),
        },
        "Item:    agent:wisent-app (the value stays in Skarbiec; read it through a grant of its own)".to_string(),
    ];
    if list {
        let client = crate::control_plane::weles::WelesClient::from_env();
        if client.health().available {
            match client.accounts(None) {
                Ok(accounts) => {
                    lines.push(format!("Weles accounts ({}):", accounts.len()));
                    for account in accounts {
                        lines.push(format!(
                            "- {} [{} · {}]",
                            account.display_name, account.provider, account.status
                        ));
                    }
                }
                Err(error) => lines.push(format!("Weles accounts unavailable: {error}")),
            }
        } else {
            lines.push(format!("Weles unavailable: {}", client.health().detail));
        }
    }
    lines.push(format!(
        "Example: curl -H \"Authorization: Bearer $(stado credentials get agent:wisent-app --field value)\" {brama}/v1/models"
    ));
    Ok(lines.join("\n") + "\n")
}

/// Slash `/token`: redacted summary only. The transcript is model-bound, so
/// the full secret is never printed here by design.
pub(crate) fn token_slash() -> Result<String, String> {
    let (brama, agent_id, secret, source) = configured()?;
    Ok(format!(
        "Agent token for Brama scripting.\nBrama: {brama}\nAgent: {agent_id}\nToken: {} (redacted, source: {source}).\nThe value stays in the Skarbiec item agent:wisent-app.\n",
        redacted(&secret)
    ))
}



