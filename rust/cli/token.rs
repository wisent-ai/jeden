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
                 `stado secrets get agent:wisent-app --field value` is how this process reads it"
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

/// CLI `jeden token [--list] [--reveal] [--json]`. `--reveal` prints the bare
/// secret on its own line so `TOKEN=$(jeden token --reveal)` stays scriptable.
pub(crate) fn token_command(args: &Args) -> Result<String, String> {
    let reveal = args.positionals.iter().any(|part| part == "--reveal");
    let list = args
        .positionals
        .iter()
        .any(|part| part == "--list" || part == "list");
    let (brama, agent_id, secret, source) = configured()?;
    if args.json {
        return Ok(format!(
            "{{\"bramaUrl\":{},\"agentId\":{},\"token\":{},\"tokenSource\":{}}}\n",
            serde_json::to_string(&brama).map_err(|error| error.to_string())?,
            serde_json::to_string(&agent_id).map_err(|error| error.to_string())?,
            serde_json::to_string(&if reveal { secret } else { redacted(&secret) })
                .map_err(|error| error.to_string())?,
            serde_json::to_string(source).map_err(|error| error.to_string())?,
        ));
    }
    if reveal {
        return Ok(format!("{secret}\n"));
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
    let (brama, agent_id, secret, source) = configured()?;
    Ok(format!(
        "Agent token for Brama scripting.\nBrama: {brama}\nAgent: {agent_id}\nToken: {} (redacted — transcript text can reach the model, source: {source}).\nPrint the full value from your shell with: jeden token --reveal\n",
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
