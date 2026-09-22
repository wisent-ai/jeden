//! Loading the declarative extensions a workspace declares, and deciding
//! which of them a prompt gets.

use serde_json::Value;

mod parse;
mod read;
mod shapes;

use parse::{collect, matches};
use read::{capability_from_result, load_agent, load_rule, load_skill};
pub(crate) use shapes::{Agent, Input, Loaded};
pub(crate) use shapes::PromptContribution;
use crate::hooks::extensions::declarative::parse::skill_file_id;
use serde_json::json;
use std::collections::BTreeSet;

const MAX_DEFINITIONS: usize = 512;
const MAX_DEFINITION_BYTES: u64 = 256 * 1024;
const MAX_ASSETS_PER_SKILL: usize = 64;

pub(super) fn load(inputs: &[Input]) -> Loaded {
    let mut loaded = Loaded::default();
    let mut ordered = inputs.to_vec();
    ordered.sort_by_key(|input| (input.precedence, input.path.clone()));
    let mut seen_files = BTreeSet::new();
    for input in ordered {
        let extensions: &[&str] = match input.kind {
            "skills" => &["md"],
            "rules" => &["md", "json", "yaml", "yml"],
            "agents" => &["json", "yaml", "yml"],
            _ => continue,
        };
        let mut files = Vec::new();
        collect(&input.path, extensions, &mut files);
        for path in files {
            if !seen_files.insert((input.kind, path.clone())) {
                continue;
            }
            match input.kind {
                "skills" => {
                    let result = load_skill(&path, input.precedence);
                    let id = result
                        .as_ref()
                        .map(|value| value.id.clone())
                        .unwrap_or_else(|_| skill_file_id(&path));
                    let description = result
                        .as_ref()
                        .map(|value| value.description.clone())
                        .unwrap_or_default();
                    let metadata = result.as_ref().map(|value| json!({"assets": value.assets, "alwaysApply": value.always_apply, "matchers": value.matchers, "promptMetadata": value.metadata})).unwrap_or(Value::Null);
                    loaded.capabilities.push(capability_from_result(
                        "skill",
                        &path,
                        &result,
                        id.clone(),
                        description,
                        metadata,
                    ));
                    if let Ok(skill) = result {
                        if loaded
                            .skills
                            .get(&id)
                            .is_none_or(|prior| prior.precedence <= skill.precedence)
                        {
                            loaded.skills.insert(id, skill);
                        }
                    }
                }
                "rules" => {
                    let result = load_rule(&path, input.precedence);
                    let id = result
                        .as_ref()
                        .map(|value| value.id.clone())
                        .unwrap_or_else(|_| {
                            path.file_stem()
                                .and_then(|value| value.to_str())
                                .unwrap_or("rule")
                                .to_string()
                        });
                    let description = result
                        .as_ref()
                        .map(|value| value.description.clone())
                        .unwrap_or_default();
                    let metadata = result.as_ref().map(|value| json!({"alwaysApply": value.always_apply, "matchers": value.matchers})).unwrap_or(Value::Null);
                    loaded.capabilities.push(capability_from_result(
                        "rule",
                        &path,
                        &result,
                        id.clone(),
                        description,
                        metadata,
                    ));
                    if let Ok(rule) = result {
                        if loaded
                            .rules
                            .get(&id)
                            .is_none_or(|prior| prior.precedence <= rule.precedence)
                        {
                            loaded.rules.insert(id, rule);
                        }
                    }
                }
                "agents" => {
                    let result = load_agent(&path, input.precedence);
                    let id = result
                        .as_ref()
                        .map(|value| value.id.clone())
                        .unwrap_or_else(|_| {
                            path.file_stem()
                                .and_then(|value| value.to_str())
                                .unwrap_or("agent")
                                .to_string()
                        });
                    let metadata = result
                        .as_ref()
                        .map(|value| value.value.clone())
                        .unwrap_or(Value::Null);
                    let description = metadata
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    loaded.capabilities.push(capability_from_result(
                        "agent",
                        &path,
                        &result,
                        id.clone(),
                        description,
                        metadata,
                    ));
                    if let Ok(agent) = result {
                        if loaded
                            .agents
                            .get(&id)
                            .is_none_or(|prior| prior.precedence <= agent.precedence)
                        {
                            loaded.agents.insert(id, agent);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    for capability in &mut loaded.capabilities {
        let winning_source = match capability.kind {
            "skill" => loaded.skills.get(&capability.id).map(|value| &value.source),
            "rule" => loaded.rules.get(&capability.id).map(|value| &value.source),
            "agent" => loaded.agents.get(&capability.id).map(|value| &value.source),
            _ => None,
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

pub(super) fn prompt_context(loaded: &Loaded, prompt: &str) -> Vec<PromptContribution> {
    let mut contributions = Vec::new();
    for rule in loaded.rules.values() {
        if rule.always_apply || matches(&rule.matchers, prompt) {
            contributions.push(PromptContribution {
                id: rule.id.clone(),
                kind: "rule",
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
                kind: "skill",
                content: skill.prompt.clone(),
                source: skill.source.clone(),
                precedence: skill.precedence,
                assets: skill.assets.clone(),
            });
        }
    }
    contributions.sort_by_key(|value| (value.precedence, value.kind, value.id.clone()));
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
            kind: "skill",
            content: skill.prompt.clone(),
            source: skill.source.clone(),
            precedence: skill.precedence,
            assets: skill.assets.clone(),
        });
    }
    Ok(out)
}
