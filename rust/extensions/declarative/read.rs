//! Turning one file on disk into a skill, a rule or an agent, and reporting
//! what happened either way.
//!
//! Split out of `extensions/declarative.rs`, which had grown past the module
//! line cap.

use super::parse::{
    parse_frontmatter, safe_assets, skill_file_id, string_list, valid_id, validate_matchers,
    Notation,
};
use super::shapes::{Agent, LoadedCapability, Rule, Skill};
use crate::task_runtime::discovery::valid_agent_id;
use crate::task_runtime::AgentDefinition;
use serde_json::Value;
use std::fs;
use std::path::Path;

/// The text `key` holds in a definition: none when it is absent or null, and
/// a refusal naming the key when it holds anything but text, so a value of
/// the wrong type is never read as absent and replaced by a default.
fn text<'a>(
    object: &'a serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<&'a str>, String> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text)),
        Some(other) => Err(format!("{key} must be text, not {other}")),
    }
}

/// The boolean `key` holds in a definition, the same way as [`text`].
fn flag(object: &serde_json::Map<String, Value>, key: &str) -> Result<Option<bool>, String> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(flag)) => Ok(Some(*flag)),
        Some(other) => Err(format!("{key} must be true or false, not {other}")),
    }
}

pub(super) fn load_skill(path: &Path, precedence: usize) -> Result<Skill, String> {
    let text_of = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let (frontmatter, body) = parse_frontmatter(&text_of)?;
    // A skill names itself by `id` or `name`; one that names neither is
    // known by its file.
    let id = match (text(&frontmatter, "id")?, text(&frontmatter, "name")?) {
        (Some(id), _) | (None, Some(id)) => id.to_string(),
        (None, None) => skill_file_id(path),
    };
    if !valid_id(&id) {
        return Err(format!("invalid skill id: {id}"));
    }
    let prompt = match text(&frontmatter, "prompt")? {
        Some(prompt) => prompt,
        None => body.as_str(),
    }
    .trim()
    .to_string();
    if prompt.is_empty() {
        return Err("skill prompt is empty".into());
    }
    // An absent description is an empty one: the skill states none.
    let description = match text(&frontmatter, "description")? {
        Some(description) => description.trim().to_string(),
        None => String::new(),
    };
    let matchers = string_list(
        frontmatter
            .get("match")
            .or_else(|| frontmatter.get("matchers")),
    )?;
    validate_matchers(&matchers)?;
    // A skill applies only where its matchers match unless it says otherwise.
    let always_apply = match flag(&frontmatter, "alwaysApply")? {
        Some(always) => always,
        None => false,
    };
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
    let text_of = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let (value, markdown_body) = match Notation::of(path) {
        Some(Notation::Structured(format)) => (format.parse::<Value>(&text_of)?, None),
        _ => {
            let (frontmatter, body) = parse_frontmatter(&text_of)?;
            (Value::Object(frontmatter), Some(body))
        }
    };
    let object = value
        .as_object()
        .ok_or("rule definition must be an object")?;
    let id = match text(object, "id")? {
        Some(id) => id.to_string(),
        None => path
            .file_stem()
            .and_then(|value| value.to_str())
            .map(str::to_string)
            .ok_or("rule id is missing and the file name is not text")?,
    };
    if !valid_id(&id) {
        return Err(format!("invalid rule id: {id}"));
    }
    // The content is `content` or `prompt`; a markdown rule may carry it as
    // its body instead.
    let content = match (
        text(object, "content")?,
        text(object, "prompt")?,
        markdown_body.as_deref(),
    ) {
        (Some(content), _, _) | (None, Some(content), _) | (None, None, Some(content)) => content,
        (None, None, None) => return Err("rule states no content or prompt".into()),
    }
    .trim()
    .to_string();
    if content.is_empty() {
        return Err("rule content is empty".into());
    }
    let matchers = string_list(object.get("match").or_else(|| object.get("matchers")))?;
    validate_matchers(&matchers)?;
    // A rule without matchers applies always unless it says otherwise.
    let always_apply = match flag(object, "alwaysApply")? {
        Some(always) => always,
        None => matchers.is_empty(),
    };
    if !always_apply && matchers.is_empty() {
        return Err("non-always rule requires at least one matcher".into());
    }
    let description = match text(object, "description")? {
        Some(description) => description.to_string(),
        None => String::new(),
    };
    Ok(Rule {
        id,
        description,
        content,
        source: path.to_path_buf(),
        precedence,
        always_apply,
        matchers,
    })
}

/// An agent definition read as delegation reads it
/// ([`crate::task_runtime::discover_agents`]): one typed shape and one id
/// rule, so a definition the loader calls healthy is one delegation takes.
pub(super) fn load_agent(path: &Path, precedence: usize) -> Result<Agent, String> {
    let text = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let Some(Notation::Structured(format)) = Notation::of(path) else {
        return Err(format!(
            "{} is not written in an agent definition format",
            path.display()
        ));
    };
    let value: Value = format.parse(&text)?;
    let definition: AgentDefinition = serde_json::from_value(value.clone())
        .map_err(|error| format!("agent definition: {error}"))?;
    if !valid_agent_id(&definition.id) {
        return Err(format!("invalid agent id: {}", definition.id));
    }
    if !definition.output.is_null() && !definition.output.is_object() {
        return Err("agent.output must be a JSON schema object".into());
    }
    Ok(Agent {
        id: definition.id,
        source: path.to_path_buf(),
        precedence,
        value,
    })
}

pub(super) fn capability_from_result<T>(
    kind: crate::capability::CapabilityKind,
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
