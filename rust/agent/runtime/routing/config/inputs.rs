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

pub(super) fn model(
    argument: Option<&str>,
    configuration: Option<&str>,
) -> Result<Option<String>, String> {
    let chosen = match argument.or(configuration) {
        Some(value) => Some(value.to_owned()),
        None => optional_env("JEDEN_MODEL")?,
    };
    Ok(chosen.filter(|value| !value.trim().is_empty()))
}

pub(super) fn agent_id(configuration: Option<&str>, direct: bool) -> Result<String, String> {
    if direct {
        return Ok(String::new());
    }
    match optional_env("WISENT_APP_AGENT_ID")? {
        Some(value) => Ok(value),
        None => match configuration {
            Some(value) => Ok(value.to_owned()),
            None => Ok("wisent-app".to_owned()),
        },
    }
}

pub(super) fn service_tier(mode: Option<String>) -> Result<String, String> {
    let configured = match optional_env("JEDEN_SERVICE_TIER")? {
        Some(value) => Some(value),
        None => optional_env("MODEL_SERVICE_TIER")?,
    };
    Ok(match configured.or(mode) {
        Some(value) => value,
        None => String::new(),
    })
}
