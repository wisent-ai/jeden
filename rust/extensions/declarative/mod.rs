//! Loading the declarative extensions a workspace declares, and deciding
//! which of them a prompt gets.

use serde_json::Value;

mod parse;
mod read;
mod shapes;

use super::loading::DeclarativeKind;
use crate::capability::CapabilityKind;
use crate::hooks::extensions::declarative::parse::skill_file_id;
use parse::{collect, matches, Notation};
use read::{capability_from_result, load_agent, load_rule, load_skill};
use serde_json::json;
pub(crate) use shapes::PromptContribution;
pub(crate) use shapes::{Agent, Input, Loaded};
use std::collections::BTreeSet;
use std::path::Path;

pub(super) fn load(inputs: &[Input]) -> Loaded {
    let mut loaded = Loaded::default();
    let mut ordered = inputs.to_vec();
    ordered.sort_by_key(|input| (input.precedence, input.path.clone()));
    let mut seen_files = BTreeSet::new();
    for input in ordered {
        // Inputs are spelled by `DeclarativeKind::name`; the registry carries
        // commands and hooks itself, so only skills, rules and agents load here.
        let Some(declared) = DeclarativeKind::named(input.kind) else {
            continue;
        };
        // Skills are Markdown, agents structured definitions, and rules
        // either.
        let accepts: fn(Notation) -> bool = match declared {
            DeclarativeKind::Skills => Notation::markdown,
            DeclarativeKind::Agents => Notation::structured,
            DeclarativeKind::Rules => |notation: Notation| notation.markdown() || notation.structured(),
            DeclarativeKind::Commands | DeclarativeKind::Hooks => continue,
        };
        let mut files = Vec::new();
        collect(&input.path, accepts, &mut files);
        for path in files {
            if !seen_files.insert((input.kind, path.clone())) {
                continue;
            }
            match declared {
                DeclarativeKind::Skills => skill(&mut loaded, &path, input.precedence),
                DeclarativeKind::Rules => rule(&mut loaded, &path, input.precedence),
                DeclarativeKind::Agents => agent(&mut loaded, &path, input.precedence),
                DeclarativeKind::Commands | DeclarativeKind::Hooks => {
                    unreachable!("the registry carries {} itself", declared.name())
                }
            }
        }
    }
    for capability in &mut loaded.capabilities {
        let winning_source = match capability.kind {
            CapabilityKind::Skill => loaded.skills.get(&capability.id).map(|value| &value.source),
            CapabilityKind::Rule => loaded.rules.get(&capability.id).map(|value| &value.source),
            CapabilityKind::Agent => loaded.agents.get(&capability.id).map(|value| &value.source),
            other => unreachable!("the declarative runtime lists skills, rules and agents, not {other}"),
        };
        capability.active = capability.healthy && winning_source == Some(&capability.path);
        if let Some(source) = winning_source.filter(|source| **source != capability.path) {
            capability.error = Some(format!(
                "shadowed by higher-precedence definition {}",
                source.display()
            ));
        }
    }
    loaded
}

/// The id a definition that could not be loaded is listed under: its file's
/// name without the extension, or the whole path when it has none.
fn file_id(path: &Path) -> String {
    match path.file_stem() {
        Some(stem) => stem.to_string_lossy().into_owned(),
        None => path.display().to_string(),
    }
}

/// One skill file: listed as a capability, healthy or with the cause it
/// failed, and kept when no definition of higher precedence holds its id.
fn skill(loaded: &mut Loaded, path: &Path, precedence: usize) {
    let result = load_skill(path, precedence);
    let (id, description, metadata) = match &result {
        Ok(skill) => (
            skill.id.clone(),
            skill.description.clone(),
            json!({"assets": skill.assets, "alwaysApply": skill.always_apply, "matchers": skill.matchers, "promptMetadata": skill.metadata}),
        ),
        Err(_cause) => (skill_file_id(path), String::new(), Value::Null),
    };
    loaded.capabilities.push(capability_from_result(
        CapabilityKind::Skill,
        path,
        &result,
        id.clone(),
        description,
        metadata,
    ));
    if let Ok(skill) = result {
        if loaded.skills.get(&id).is_none_or(|prior| prior.precedence <= skill.precedence) {
            loaded.skills.insert(id, skill);
        }
    }
}

/// One rule file, listed and kept as a skill is.
fn rule(loaded: &mut Loaded, path: &Path, precedence: usize) {
    let result = load_rule(path, precedence);
    let (id, description, metadata) = match &result {
        Ok(rule) => (
            rule.id.clone(),
            rule.description.clone(),
            json!({"alwaysApply": rule.always_apply, "matchers": rule.matchers}),
        ),
        Err(_cause) => (file_id(path), String::new(), Value::Null),
    };
    loaded.capabilities.push(capability_from_result(
        CapabilityKind::Rule,
        path,
        &result,
        id.clone(),
        description,
        metadata,
    ));
    if let Ok(rule) = result {
        if loaded.rules.get(&id).is_none_or(|prior| prior.precedence <= rule.precedence) {
            loaded.rules.insert(id, rule);
        }
    }
}

/// One agent definition, listed and kept as a skill is; its description is
/// the definition's own, and a definition that states none has none.
fn agent(loaded: &mut Loaded, path: &Path, precedence: usize) {
    let result = load_agent(path, precedence);
    let (id, description, metadata) = match &result {
        Ok(agent) => {
            let description = match agent.value.get("description") {
                None => String::new(),
                Some(Value::String(text)) => text.clone(),
                Some(other) => other.to_string(),
            };
            (agent.id.clone(), description, agent.value.clone())
        }
        Err(_cause) => (file_id(path), String::new(), Value::Null),
    };
    loaded.capabilities.push(capability_from_result(
        CapabilityKind::Agent,
        path,
        &result,
        id.clone(),
        description,
        metadata,
    ));
    if let Ok(agent) = result {
        if loaded.agents.get(&id).is_none_or(|prior| prior.precedence <= agent.precedence) {
            loaded.agents.insert(id, agent);
        }
    }
}

pub(super) fn prompt_context(loaded: &Loaded, prompt: &str) -> Vec<PromptContribution> {
    let mut contributions = Vec::new();
    for rule in loaded.rules.values() {
        if rule.always_apply || matches(&rule.matchers, prompt) {
            contributions.push(PromptContribution {
                id: rule.id.clone(),
                kind: CapabilityKind::Rule,
                content: rule.content.clone(),
                source: rule.source.clone(),
                precedence: rule.precedence,
                assets: Vec::new(),
            });
        }
    }
    for skill in loaded.skills.values() {
        if skill.always_apply || matches(&skill.matchers, prompt) {
            contributions.push(PromptContribution {
                id: skill.id.clone(),
                kind: CapabilityKind::Skill,
                content: skill.prompt.clone(),
                source: skill.source.clone(),
                precedence: skill.precedence,
                assets: skill.assets.clone(),
            });
        }
    }
    contributions.sort_by_key(|value| (value.precedence, value.kind.name(), value.id.clone()));
    contributions
}

pub(super) fn skill_context(
    loaded: &Loaded,
    ids: &[String],
) -> Result<Vec<PromptContribution>, String> {
    let mut out = Vec::new();
    for id in ids {
        let skill = loaded
            .skills
            .get(id)
            .ok_or_else(|| format!("active skill not found: {id}"))?;
        out.push(PromptContribution {
            id: skill.id.clone(),
            kind: CapabilityKind::Skill,
            content: skill.prompt.clone(),
            source: skill.source.clone(),
            precedence: skill.precedence,
            assets: skill.assets.clone(),
        });
    }
    Ok(out)
}
