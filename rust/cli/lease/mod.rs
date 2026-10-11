//! `jeden lease`: the Brama accounts this agent's sessions run on.
//!
//! The register is Brama's (`/v1/subscription-pool/leases`); a session takes
//! its lease on its first leased call and gives it back when it ends
//! (`crate::model_router::lease`). This command reads the register as Jeden's
//! agent sees it and ends a lease a session left behind, for instance when
//! its process was killed before it could release.

use crate::model_router::lease::{self, SessionLease};
use crate::model_router::ChatConfig;
use crate::Args;

const USAGE: &str = "jeden lease [list] [--json] — the Brama accounts this agent's sessions are leased on, each provider's sessions-per-account limit and how many sessions each account carries
jeden lease release <lease-id> [--json] — end one lease
jeden lease end <session-id> [--json] — end every lease of one session";

pub(crate) fn command(args: &Args) -> Result<String, String> {
    let words: Vec<&str> = args.positionals.iter().map(String::as_str).collect();
    match words.as_slice() {
        [] | ["list"] => list(&gateway(args)?, args.json),
        ["release", lease_id] => released(lease::release(&gateway(args)?, lease_id)?, args.json),
        ["end", session_id] => released(
            lease::release_session(&gateway(args)?, session_id)?,
            args.json,
        ),
        _ => Err(crate::cli::invocation::refusal::usage(format!(
            "jeden lease takes one of:\n{USAGE}"
        ))),
    }
}

/// Brama as this agent reaches it for model calls: the same address,
/// bearer and signing secret. A configuration that does not reach Brama
/// has no lease to read, and says why.
fn gateway(args: &Args) -> Result<ChatConfig, String> {
    let config = crate::agent::model_router_config(&crate::load_config(&args.cwd), args);
    if !config.via_brama {
        return Err(format!(
            "{} names a provider called without Brama; it keeps no pool of accounts, so no session holds a lease",
            crate::agent::credential::DIRECT_URL
        ));
    }
    if config.url.trim().is_empty() || config.secret.is_empty() {
        let cause = config.config_error.clone().ok_or(
            "the router configuration names no Brama address or signing secret and gives no cause",
        )?;
        return Err(format!(
            "Brama cannot be reached to read the lease register: {cause}"
        ));
    }
    Ok(config)
}

fn line(lease: &SessionLease) -> String {
    format!(
        "{}  {}  {}  session {}  held by {}  taken at {} ms since the epoch",
        lease.id,
        lease.provider,
        lease.member(),
        lease.session_id,
        lease.holder,
        lease.taken_at_ms
    )
}

fn list(config: &ChatConfig, json: bool) -> Result<String, String> {
    let register = lease::register(config)?;
    let capacity = lease::capacity(config, &register.leases)?;
    if json {
        let mut document = serde_json::to_value(&register)
            .map_err(|error| format!("the lease register cannot be printed as JSON: {error}"))?;
        document["capacity"] = serde_json::json!(capacity);
        return serde_json::to_string_pretty(&document)
            .map(|text| text + "\n")
            .map_err(|error| format!("the lease register cannot be printed as JSON: {error}"));
    }
    let mut out = String::new();
    if register.leases.is_empty() {
        out.push_str("no live lease of this agent\n");
    }
    for lease in &register.leases {
        out.push_str(&line(lease));
        out.push('\n');
    }
    for (provider, limit) in &register.limits {
        out.push_str(&format!(
            "{provider}: at most {limit} sessions per account\n"
        ));
    }
    for (provider, refusal) in &register.unstated {
        out.push_str(&format!(
            "{provider}: no sessions limit stated, so no lease: {refusal}\n"
        ));
    }
    for (subscription, sessions) in &register.counts {
        out.push_str(&format!("{subscription}: carries {sessions} sessions\n"));
    }
    match &capacity {
        None => out.push_str(
            "what each account has left: not stated; Brama's pool document carries no accounts.capacity\n",
        ),
        Some(accounts) => {
            for (subscription, account) in accounts {
                out.push_str(&format!(
                    "{subscription}: available={} tightest remaining={} live sessions={}\n",
                    account["available"],
                    account["tightest_remaining_fraction"],
                    account["live_sessions"]
                ));
                for window in account["windows"].as_array().into_iter().flatten() {
                    out.push_str(&format!(
                        "    {}: used={} remaining={} resets_at_ms={} current={}\n",
                        window["label"],
                        window["used_fraction"],
                        window["remaining_fraction"],
                        window["resets_at_ms"],
                        window["current"]
                    ));
                }
            }
        }
    }
    Ok(out)
}

fn released(leases: Vec<SessionLease>, json: bool) -> Result<String, String> {
    if json {
        return serde_json::to_string_pretty(&serde_json::json!({ "released": leases }))
            .map(|text| text + "\n")
            .map_err(|error| format!("the released leases cannot be printed as JSON: {error}"));
    }
    if leases.is_empty() {
        return Ok("nothing released: no live lease of this agent matched\n".into());
    }
    Ok(leases
        .iter()
        .map(|lease| format!("released {}\n", line(lease)))
        .collect())
}
