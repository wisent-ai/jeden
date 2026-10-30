//! `jeden token`: manage local model credentials without printing secret values.
use crate::agent::credential::{self, tokens::Store, Source};
use crate::Args;
use serde_json::json;
use std::io::{IsTerminal, Read};

fn rows() -> Result<Vec<credential::tokens::TokenRow>, String> {
    match Store::open_existing()? {
        Some(store) => store.list(),
        None => Ok(Vec::new()),
    }
}

fn format_rows(tokens: &[credential::tokens::TokenRow]) -> String {
    let mut text = String::new();
    for token in tokens {
        text.push_str(&format!(
            "{} {} {}{}\n",
            token.id,
            token.label,
            token.endpoint,
            if token.active { " [last selected]" } else { "" }
        ));
        if let Some(failure) = &token.last_failure {
            text.push_str(&format!("  last failure: {failure}\n"));
        }
    }
    text
}

fn status(json_output: bool) -> Result<String, String> {
    let tokens = rows()?;
    let direct = credential::direct_provider();
    if let Some(cause) = direct.is_none().then(credential::direct_refusal).flatten() {
        if json_output {
            return serde_json::to_string(&json!({
                "route": "direct", "endpoint": null, "keySource": "refused",
                "selected": null, "routeRefusal": cause, "tokens": tokens,
            }))
            .map(|text| format!("{text}\n"))
            .map_err(|error| error.to_string());
        }
        return Ok(format!(
            "Direct provider refused: {cause}\nKey source: refused\n{}",
            format_rows(&tokens)
        ));
    }
    let active_endpoint = direct.as_ref().map(|(endpoint, _)| endpoint.as_str());
    let stored = active_endpoint
        .is_some_and(|endpoint| tokens.iter().any(|token| token.endpoint == endpoint));
    let key_source = match &direct {
        Some((_, Some(_))) if credential::standing_in() == Some("stado-resolved") => {
            "stado-resolved"
        }
        Some((_, Some(_))) if credential::standing_in().is_some() => "own-gateway",
        Some((_, Some(_))) => "environment",
        Some(_) if stored => "local-token-list",
        Some(_) => "none",
        None => "not-needed",
    };
    let selected = if key_source == "local-token-list" {
        tokens
            .iter()
            .find(|token| token.active && Some(token.endpoint.as_str()) == active_endpoint)
            .map(|token| token.id.as_str())
    } else {
        None
    };
    if json_output {
        let value = match &direct {
            Some((endpoint, _)) => json!({
                "route": "direct", "endpoint": endpoint,
                "keySource": key_source,
                "selected": selected,
                "tokens": tokens,
            }),
            None => {
                let route = credential::brama_route();
                let (secret, bearer) = credential::ensure();
                json!({
                    "route": "brama", "endpoint": route.url,
                    "origin": credential::route_origin(),
                    "signingCredential": secret.as_str(), "gatewayBearer": bearer.as_str(),
                    "keySource": bearer.as_str(), "selected": selected,
                    "signingRefusal": secret.refusal(), "bearerRefusal": bearer.refusal(),
                    "ownGateway": {
                        "grantsFile": credential::private_gateway::grants_file(),
                        "state": credential::private_gateway::described(),
                    },
                    "tokens": tokens,
                })
            }
        };
        return serde_json::to_string(&value)
            .map(|text| format!("{text}\n"))
            .map_err(|error| error.to_string());
    }
    let mut text = match &direct {
        Some((endpoint, _)) => format!("Direct provider: {endpoint}\nKey source: {key_source}\n"),
        None => {
            let route = credential::brama_route();
            let endpoint = match (route.url.as_deref(), &route.source) {
                (Some(url), _) => url.to_string(),
                (
                    None,
                    Source::Refused(cause)
                    | Source::EnvironmentRefused(cause)
                    | Source::DirectRefused(cause),
                ) => format!("unavailable: {cause}"),
                (None, source) => {
                    return Err(format!(
                        "Brama route has no endpoint (source: {})",
                        source.as_str()
                    ))
                }
            };
            let (secret, bearer) = credential::ensure();
            let mut text = format!(
                "Brama: {endpoint}\nOrigin: {}\nSigning credential: {}\nGateway bearer: {}\n",
                credential::route_origin(),
                secret.as_str(),
                bearer.as_str()
            );
            if let Some(cause) = secret.refusal() {
                text.push_str(&format!("Signing refusal: {cause}\n"));
            }
            if let Some(cause) = bearer.refusal() {
                text.push_str(&format!("Bearer refusal: {cause}\n"));
            }
            text.push_str(&format!(
                "Jeden's own gateway: {}\n",
                credential::private_gateway::described()
            ));
            text
        }
    };
    text.push_str(&format_rows(&tokens));
    Ok(text)
}

fn list(json_output: bool) -> Result<String, String> {
    let tokens = rows()?;
    if json_output {
        return serde_json::to_string(&json!({ "tokens": tokens }))
            .map(|text| format!("{text}\n"))
            .map_err(|error| error.to_string());
    }
    Ok(format_rows(&tokens))
}

pub(crate) fn token_command(args: &Args) -> Result<String, String> {
    match args.positionals.as_slice() {
        [] => status(args.json),
        [verb] if verb == "status" => status(args.json),
        [verb] if verb == "list" => list(args.json),
        [verb, label, flag, endpoint] if verb == "add" && flag == "--endpoint" => {
            if std::io::stdin().is_terminal() {
                return Err(crate::cli::invocation::refusal::usage(
                    "token add reads the key from stdin; pipe it instead of placing it on argv"));
            }
            let mut secret = String::new();
            std::io::stdin().read_to_string(&mut secret)
                .map_err(|error| format!("read model token from stdin: {error}"))?;
            let row = Store::add(label, endpoint, &secret)?;
            if args.json {
                serde_json::to_string(&row).map(|text| format!("{text}\n"))
                    .map_err(|error| error.to_string())
            } else {
                Ok(format!("stored token {} for {} (id {})\n", row.label, row.endpoint, row.id))
            }
        }
        [verb, id] if verb == "remove" => {
            Store::remove_existing(id)?;
            if args.json { Ok(json!({"removed": id}).to_string() + "\n") }
            else { Ok(format!("removed token {id}\n")) }
        }
        [verb, provider, account_flag, account, reason_flag, reason]
            if verb == "sign-in" && account_flag == "--account" && reason_flag == "--reason" =>
        {
            let verdict = credential::private_gateway::sign_in(provider, account, reason)?;
            if args.json {
                Ok(format!("{verdict}\n"))
            } else {
                Ok(format!(
                    "signed {account}'s {provider} subscription into Jeden's own gateway ({}): {verdict}\n",
                    credential::private_gateway::grants_file().display()
                ))
            }
        }
        _ => Err(crate::cli::invocation::refusal::usage(
            "usage: jeden token [status|list|add LABEL --endpoint URL|remove ID|sign-in PROVIDER --account EMAIL --reason TEXT] [--json]; add reads the secret from stdin, sign-in the code the provider's page shows")),
    }
}

pub(crate) fn token_slash() -> Result<String, String> {
    status(false)
}
