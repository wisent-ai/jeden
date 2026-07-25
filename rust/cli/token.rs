//! `jeden token` — print the agent's own Brama credential for scripting
//! (curl examples, CI jobs). Provider OAuth tokens live in Skarbiec/Brama and
//! are never held by jeden, so the agent auth secret is the only credential
//! jeden can print. Values are redacted by default; `--reveal` prints the full
//! secret to the user's shell. The `/token` slash form never reveals —
//! transcript text can reach the model, and `secrets.mode` protects exactly
//! that path.

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

fn brama_url() -> String {
    env::var("BRAMA_URL")
        .or_else(|_| env::var("WISENT_MODEL_ROUTER_URL"))
        .unwrap_or_default()
}

fn configured() -> Result<(String, String, String), String> {
    let secret = env::var(SECRET_KEY).unwrap_or_default();
    if secret.is_empty() {
        return Err(format!(
            "{SECRET_KEY} is not configured; run /setup to store it in ~/.jeden/.env"
        ));
    }
    Ok((brama_url(), env::var(AGENT_ID_KEY).unwrap_or_default(), secret))
}

/// CLI `jeden token [--list] [--reveal] [--json]`. `--reveal` prints the bare
/// secret on its own line so `TOKEN=$(jeden token --reveal)` stays scriptable.
pub(crate) fn token_command(args: &Args) -> Result<String, String> {
    let reveal = args.positionals.iter().any(|part| part == "--reveal");
    let list = args
        .positionals
        .iter()
        .any(|part| part == "--list" || part == "list");
    let (brama, agent_id, secret) = configured()?;
    if args.json {
        return Ok(format!(
            "{{\"bramaUrl\":{},\"agentId\":{},\"token\":{}}}\n",
            serde_json::to_string(&brama).map_err(|error| error.to_string())?,
            serde_json::to_string(&agent_id).map_err(|error| error.to_string())?,
            serde_json::to_string(&if reveal { secret } else { redacted(&secret) })
                .map_err(|error| error.to_string())?,
        ));
    }
    if reveal {
        return Ok(format!("{secret}\n"));
    }
    let mut lines = vec![
        format!("Brama:   {brama}"),
        format!("Agent:   {agent_id}"),
        format!("Token:   {} — stored in ~/.jeden/.env", redacted(&secret)),
        "Reveal:  jeden token --reveal (prints the bare value for scripting)".to_string(),
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
        "Example: curl -H \"Authorization: Bearer $(jeden token --reveal)\" {brama}/v1/models"
    ));
    Ok(lines.join("\n") + "\n")
}

/// Slash `/token`: redacted summary only. The transcript is model-bound, so
/// the full secret is never printed here by design.
pub(crate) fn token_slash() -> Result<String, String> {
    let (brama, agent_id, secret) = configured()?;
    Ok(format!(
        "Agent token for Brama scripting.\nBrama: {brama}\nAgent: {agent_id}\nToken: {} (redacted — transcript text can reach the model).\nPrint the full value from your shell with: jeden token --reveal\n",
        redacted(&secret)
    ))
}

#[cfg(test)]
mod tests {
    #[test]
    fn redaction_keeps_only_the_tail() {
        assert_eq!(super::redacted("abcdefghij"), "…ghij (10 chars)");
        assert_eq!(super::redacted("abcd"), "…abcd (4 chars)");
    }
}
