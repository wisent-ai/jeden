use std::env;
use std::path::Path;

use crate::control_plane::billing::{QuotaBucket, QuotaState};
use crate::control_plane::contract::{RequestMeta, WelesApiV2};

use crate::control_plane::weles::{InteractionBridge, WelesClient};
use crate::load_config;
use crate::tui::{PickerItem, PickerSpec};

mod bridges;

use bridges::ConsoleBridge;
pub(crate) use bridges::TurnBridge;

fn route_key(value: &str) -> Option<String> {
    let key = value.trim().to_ascii_lowercase();
    let mut chars = key.chars();
    let first_ok = matches!(chars.next(), Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit());
    (first_ok
        && chars
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-')))
    .then_some(key)
}

fn item_slug(value: &str) -> String {
    value
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

/// The agent Weles logs in for: WISENT_APP_AGENT_ID, then the configured
/// agent id, then the declared default, the order routing reads it in. A
/// variable that is not Unicode is refused, never read as unset.
pub(crate) fn agent_identity(cwd: &Path) -> Result<String, String> {
    match env::var("WISENT_APP_AGENT_ID") {
        Ok(value) if !value.trim().is_empty() => return Ok(value),
        Ok(_) | Err(env::VarError::NotPresent) => {}
        Err(env::VarError::NotUnicode(_)) => {
            return Err("WISENT_APP_AGENT_ID cannot be read: value is not Unicode".into())
        }
    }
    Ok(match load_config(cwd).agent_id {
        Some(configured) => configured,
        None => crate::slash::setup::DEFAULT_AGENT_ID.to_string(),
    })
}

pub(crate) fn format_auth_status(cwd: &Path) -> String {
    let client = WelesClient::from_env();
    let health = client.health();
    let mut lines = vec![
        "Jeden authentication status".to_string(),
        format!("Workspace: {}", cwd.display()),
        match agent_identity(cwd) {
            Ok(agent) => format!("Agent: {agent}"),
            Err(cause) => format!("Agent: unreadable: {cause}"),
        },
        format!(
            "Weles {}: {}",
            health.version,
            if health.available {
                "available"
            } else {
                "unavailable"
            }
        ),
        format!("Health: {}", health.detail),
    ];
    if health.available {
        match client.refresh_due(&ConsoleBridge, &|| false) {
            Ok(refreshed) if !refreshed.is_empty() => lines.push(format!(
                "Automatically refreshed {} expiring account(s).",
                refreshed.len()
            )),
            Err(error) => lines.push(format!("Automatic refresh failed: {error}")),
            _ => {}
        }
        match client.accounts(None) {
            Ok(accounts) if accounts.is_empty() => lines.push("Accounts: none".into()),
            Ok(accounts) => {
                lines.push("Accounts and subscriptions:".into());
                for (account_index, account) in accounts.into_iter().enumerate() {
                    lines.push(format!(
                        "  {} ({}) — {}",
                        account.display_name, account.provider, account.status
                    ));
                    let correlation = format!("auth-status-{account_index}");
                    match client.subscriptions(
                        &account.id,
                        &RequestMeta::read_v2(format!("{correlation}-subscriptions")),
                    ) {
                        Ok(subscriptions) if subscriptions.is_empty() => {
                            lines.push("    subscriptions: none".into());
                        }
                        Ok(subscriptions) => {
                            for (subscription_index, subscription) in
                                subscriptions.into_iter().enumerate()
                            {
                                lines.push(format!(
                                    "    {} — {}",
                                    subscription.product_id,
                                    format!("{:?}", subscription.status).to_ascii_lowercase()
                                ));
                                match client.quota(
                                    &subscription.id,
                                    &RequestMeta::read_v2(format!(
                                        "{correlation}-quota-{subscription_index}"
                                    )),
                                ) {
                                    Ok(quota) if quota.buckets.is_empty() => {
                                        lines.push("      quota: not reported".into());
                                    }
                                    Ok(quota) => lines.extend(
                                        quota
                                            .buckets
                                            .iter()
                                            .map(|bucket| format!("      {}", quota_line(bucket))),
                                    ),
                                    Err(error) => {
                                        lines.push(format!("      quota unavailable: {error}"));
                                    }
                                }
                            }
                        }
                        Err(error) => {
                            lines.push(format!("    subscription discovery failed: {error}"));
                        }
                    }
                }
            }
            Err(error) => lines.push(format!("Account discovery failed: {error}")),
        }
    }
    lines.join("\n")
}

fn quota_line(bucket: &QuotaBucket) -> String {
    let amount = match (bucket.remaining, bucket.limit) {
        (Some(remaining), Some(limit)) => format!("{remaining}/{limit} remaining"),
        (Some(remaining), None) => format!("{remaining} remaining"),
        (None, Some(limit)) => format!("limit {limit}"),
        (None, None) if bucket.state == QuotaState::Unmetered => "unmetered".into(),
        (None, None) => "amount not reported".into(),
    };
    format!(
        "{}: {} ({amount})",
        bucket.bucket_id,
        format!("{:?}", bucket.state).to_ascii_lowercase()
    )
}

pub(crate) fn provider_picker(cwd: &Path) -> Result<PickerSpec, String> {
    let lang = crate::cli::i18n::lang_code(cwd);
    let client = WelesClient::from_env();
    if !client.health().available {
        return Err(client.health().detail);
    }
    let providers = crate::control_plane::providers(cwd, &client)
        .map_err(|error| format!("Weles could not list its login providers: {error}"))?;
    let items = providers
        .into_iter()
        .map(|provider| {
            let methods = provider
                .login_methods
                .iter()
                .map(|method| format!("{method:?}"))
                .collect::<Vec<_>>()
                .join(", ");
            PickerItem::action(&provider.display_name, format!("/login {}", provider.id))
                .detail(if provider.available {
                    format!("Weles: {methods}")
                } else {
                    match provider.unavailable_reason {
                        Some(reason) => reason,
                        None => "Unavailable; Weles named no reason".into(),
                    }
                })
                .badge(if provider.available {
                    crate::cli::i18n::tr(&lang, "badge.available")
                } else {
                    crate::cli::i18n::tr(&lang, "badge.unavailable")
                })
                .disabled(!provider.available)
        })
        .collect();
    Ok(PickerSpec::new("Select provider to login", items))
}

pub(crate) fn start_login_with_bridge(
    cwd: &Path,
    args: &str,
    bridge: &dyn InteractionBridge,
    cancelled: &dyn Fn() -> bool,
) -> Result<String, String> {
    let provider = route_key(args).ok_or("Usage: /login <provider advertised by Weles>")?;
    let client = WelesClient::from_env();
    let advertised = crate::control_plane::providers(cwd, &client)
        .map_err(|error| format!("Weles could not list its login providers: {error}"))?
        .into_iter()
        .find(|entry| entry.id == provider)
        .ok_or_else(|| format!("provider `{provider}` is not registered"))?;
    let account = client
        .login_provider(
            &advertised,
            &format!("jeden:{}", item_slug(&agent_identity(cwd)?)),
            bridge,
            cancelled,
        )
        .map_err(|error| format!("Weles could not log in to {provider}: {error}"))?;
    Ok(match account {
        Some(value) => format!("Logged in to {} as {}.", value.provider, value.display_name),
        None => format!("Logged in to {provider}; Weles returned no account record for it."),
    })
}

pub(crate) fn start_login(cwd: &Path, args: &str) -> Result<String, String> {
    if args.trim().is_empty() {
        return Ok(format_auth_status(cwd));
    }
    start_login_with_bridge(cwd, args, &ConsoleBridge, &|| false)
}

fn account_id(client: &WelesClient, selector: &str) -> Result<String, String> {
    let key = route_key(selector).ok_or("account id or provider is required")?;
    client
        .accounts(None)
        .map_err(|error| format!("Weles could not list its accounts: {error}"))?
        .into_iter()
        .find(|account| account.id == key || account.provider == key)
        .map(|account| account.id)
        .ok_or_else(|| format!("no Weles account matches `{key}`"))
}

pub(crate) fn refresh(args: &str) -> Result<String, String> {
    let client = WelesClient::from_env();
    if args.trim().is_empty() {
        let refreshed = client
            .refresh_due(&ConsoleBridge, &|| false)
            .map_err(|error| format!("Weles could not refresh its expiring accounts: {error}"))?;
        return Ok(format!(
            "Refreshed {} expiring Weles account(s).",
            refreshed.len()
        ));
    }
    let id = account_id(&client, args)?;
    let account = client
        .refresh(&id, &ConsoleBridge, &|| false)
        .map_err(|error| format!("Weles could not refresh account {id}: {error}"))?;
    Ok(match account {
        Some(value) => format!("Refreshed {} as {}.", value.provider, value.display_name),
        None => format!("Refreshed Weles account {id}; Weles returned no account record for it."),
    })
}

pub(crate) fn logout(cwd: &Path, args: &str) -> Result<String, String> {
    let client = WelesClient::from_env();
    let account_id = account_id(&client, args)
        .map_err(|cause| format!("{cause}; usage: /logout <account-id or provider>"))?;
    client
        .logout(&account_id, &ConsoleBridge, &|| false)
        .map_err(|error| format!("Weles could not log out account {account_id}: {error}"))?;
    Ok(format!(
        "Logged out Weles account {account_id} for agent {}.",
        agent_identity(cwd)?
    ))
}
