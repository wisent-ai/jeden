//! Process routing inputs retain malformed and non-Unicode values as refusals.
use std::env;

fn optional_env(name: &str) -> Result<Option<String>, String> {
    match env::var(name) {
        Ok(value) if !value.trim().is_empty() => Ok(Some(value)),
        Ok(_) | Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => {
            Err(format!("{name} cannot be read: value is not Unicode"))
        }
    }
}

/// The model a run routes to, the one order every reader shares: the run's
/// own choice (`--model`, or `/model` for this session), then `JEDEN_MODEL`
/// for this run, then the configured `model` that `/setup` records. A blank
/// value selects nothing; a `JEDEN_MODEL` that is not Unicode is refused.
/// `None` is no model selected.
pub(crate) fn selected_model(
    argument: Option<&str>,
    configuration: Option<&str>,
) -> Result<Option<String>, String> {
    let chosen = |value: Option<&str>| {
        value
            .filter(|value| !value.trim().is_empty())
            .map(str::to_owned)
    };
    if let Some(model) = chosen(argument) {
        return Ok(Some(model));
    }
    if let Some(model) = optional_env("JEDEN_MODEL")? {
        return Ok(Some(model));
    }
    Ok(chosen(configuration))
}

/// The agent a Brama route signs as (none on a direct provider route), read
/// through the one order [`crate::agent::credential::agent_id`] declares.
pub(super) fn agent_id(configuration: Option<&str>, direct: bool) -> Result<String, String> {
    if direct {
        return Ok(String::new());
    }
    crate::agent::credential::agent_id(configuration)
}

/// The service tier a model call carries, the one order every reader shares:
/// `JEDEN_SERVICE_TIER`, then `MODEL_SERVICE_TIER`, then the session's fast
/// mode tier (`mode`, present only while fast mode is on). `None` is no tier
/// stated; a variable that is not Unicode is refused naming it.
pub(crate) fn selected_service_tier(mode: Option<String>) -> Result<Option<String>, String> {
    let configured = match optional_env("JEDEN_SERVICE_TIER")? {
        Some(value) => Some(value),
        None => optional_env("MODEL_SERVICE_TIER")?,
    };
    Ok(configured.or(mode))
}

/// The tier as the request writes it: no tier stated is sent as none.
pub(super) fn service_tier(mode: Option<String>) -> Result<String, String> {
    Ok(match selected_service_tier(mode)? {
        Some(value) => value,
        None => String::new(),
    })
}
