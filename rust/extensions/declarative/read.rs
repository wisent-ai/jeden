//! Turning one file on disk into a skill, a rule or an agent, and reporting
//! what happened either way.
//!
//! Split out of `extensions/declarative.rs`, which had grown past the module
//! line cap.

use super::parse::{
    parse_frontmatter, safe_assets, skill_file_id, string_list, valid_id, validate_matchers,
};
use super::shapes::{Agent, LoadedCapability, Rule, Skill};
use super::MAX_DEFINITION_BYTES;
use serde_json::Value;
use std::fs;
use std::path::Path;

pub(super) fn load_skill(path: &Path, precedence: usize) -> Result<Skill, String> {
    let metadata = fs::metadata(path).map_err(|error| error.to_string())?;
    if metadata.len() > MAX_DEFINITION_BYTES {
        return Err("skill definition exceeds 256 KiB".into());
    }
    let text = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let (frontmatter, body) = parse_frontmatter(&text)?;
    let id = frontmatter
        .get("id")
        .or_else(|| frontmatter.get("name"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| skill_file_id(path));
    if !valid_id(&id) {
        return Err(format!("invalid skill id: {id}"));
    }
    let prompt = frontmatter
        .get("prompt")
        .and_then(Value::as_str)
        .unwrap_or(&body)
        .trim()
        .to_string();
    if prompt.is_empty() {
        return Err("skill prompt is empty".into());
    }
    let description = frontmatter
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let matchers = string_list(
        frontmatter
            .get("match")
            .or_else(|| frontmatter.get("matchers")),
    )?;
    validate_matchers(&matchers)?;
    let always_apply = frontmatter
        .get("alwaysApply")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let assets = safe_assets(path, frontmatter.get("assets"))?;
    Ok(Skill {
        id,
        description,
        prompt,
        source: path.to_path_buf(),
        precedence,
        always_apply,
        matchers,
        assets,
        metadata: Value::Object(frontmatter),
    })
}

pub(super) fn load_rule(path: &Path, precedence: usize) -> Result<Rule, String> {
    let metadata = fs::metadata(path).map_err(|error| error.to_string())?;
    if metadata.len() > MAX_DEFINITION_BYTES {
        return Err("rule definition exceeds 256 KiB".into());
    }
    let text = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let (value, fallback_body) = match path.extension().and_then(|value| value.to_str()) {
        Some("json") => (
            serde_json::from_str::<Value>(&text).map_err(|error| error.to_string())?,
            String::new(),
        ),
        Some("yaml" | "yml") => (
            serde_yaml::from_str::<Value>(&text).map_err(|error| error.to_string())?,
            String::new(),
        ),
        _ => {
            let (frontmatter, body) = parse_frontmatter(&text)?;
            (Value::Object(frontmatter), body)
        }
    };
    let object = value
        .as_object()
        .ok_or("rule definition must be an object")?;
    let id = object
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            path.file_stem()
                .and_then(|value| value.to_str())
                .map(str::to_string)
        })
        .ok_or("rule id is missing")?;
    if !valid_id(&id) {
        return Err(format!("invalid rule id: {id}"));
    }
    let content = object
        .get("content")
        .or_else(|| object.get("prompt"))
        .and_then(Value::as_str)
        .unwrap_or(&fallback_body)
        .trim()
        .to_string();
    if content.is_empty() {
        return Err("rule content is empty".into());
    }
    let matchers = string_list(object.get("match").or_else(|| object.get("matchers")))?;
    validate_matchers(&matchers)?;
    let always_apply = object
        .get("alwaysApply")
        .and_then(Value::as_bool)
        .unwrap_or(matchers.is_empty());
    if !always_apply && matchers.is_empty() {
        return Err("non-always rule requires at least one matcher".into());
    }
    Ok(Rule {
        id,
        description: object
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        content,
        source: path.to_path_buf(),
        precedence,
        always_apply,
        matchers,
    })
}

pub(super) fn load_agent(path: &Path, precedence: usize) -> Result<Agent, String> {
    let metadata = fs::metadata(path).map_err(|error| error.to_string())?;
    if metadata.len() > MAX_DEFINITION_BYTES {
        return Err("agent definition exceeds 256 KiB".into());
    }
    let text = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let value = match path.extension().and_then(|value| value.to_str()) {
        Some("yaml" | "yml") => {
            serde_yaml::from_str::<Value>(&text).map_err(|error| error.to_string())?
        }
        _ => serde_json::from_str::<Value>(&text).map_err(|error| error.to_string())?,
    };
    let object = value
        .as_object()
        .ok_or("agent definition must be an object")?;
    let id = object
        .get("id")
        .and_then(Value::as_str)
        .ok_or("agent id is missing")?
        .to_string();
    if !valid_id(&id) {
        return Err(format!("invalid agent id: {id}"));
    }
    for key in ["description", "prompt", "model"] {
        if object
            .get(key)
            .is_some_and(|value| !value.is_null() && !value.is_string())
        {
            return Err(format!("agent.{key} must be a string"));
        }
    }
    for key in ["tools", "skills"] {
        string_list(object.get(key)).map_err(|error| format!("agent.{key}: {error}"))?;
    }
    if let Some(spawn) = object.get("spawn") {
        let spawn = spawn.as_object().ok_or("agent.spawn must be an object")?;
        string_list(spawn.get("allowAgents"))
            .map_err(|error| format!("agent.spawn.allowAgents: {error}"))?;
        string_list(spawn.get("denyAgents"))
            .map_err(|error| format!("agent.spawn.denyAgents: {error}"))?;
        if spawn
            .get("allowRecursive")
            .is_some_and(|value| !value.is_boolean())
        {
            return Err("agent.spawn.allowRecursive must be boolean".into());
        }
    }
    if object
        .get("output")
        .is_some_and(|value| !value.is_null() && !value.is_object())
    {
        return Err("agent.output must be a JSON schema object".into());
    }
    Ok(Agent {
        id,
        source: path.to_path_buf(),
        precedence,
        value,
    })
}

pub(super) fn capability_from_result<T>(
    kind: &'static str,
    path: &Path,
    result: &Result<T, String>,
    id: String,
    description: String,
    metadata: Value,
) -> LoadedCapability {
    LoadedCapability {
        kind,
        active: result.is_ok(),
        id,
        path: path.to_path_buf(),
        healthy: result.is_ok(),
        error: result.as_ref().err().cloned(),
        description,
        metadata,
    }
}
