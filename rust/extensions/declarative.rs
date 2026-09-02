use regex::Regex;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

const MAX_DEFINITIONS: usize = 512;
const MAX_DEFINITION_BYTES: u64 = 256 * 1024;
const MAX_ASSETS_PER_SKILL: usize = 64;

#[derive(Clone, Debug)]
pub(super) struct Input {
    pub kind: &'static str,
    pub path: PathBuf,
    pub precedence: usize,
}

#[derive(Clone, Debug)]
pub(super) struct LoadedCapability {
    pub kind: &'static str,
    pub active: bool,
    pub id: String,
    pub path: PathBuf,
    pub healthy: bool,
    pub error: Option<String>,
    pub description: String,
    pub metadata: Value,
}

#[derive(Clone, Debug)]
pub(super) struct Skill {
    pub id: String,
    pub description: String,
    pub prompt: String,
    pub source: PathBuf,
    pub precedence: usize,
    pub always_apply: bool,
    pub matchers: Vec<String>,
    pub assets: Vec<PathBuf>,
    pub metadata: Value,
}

#[derive(Clone, Debug)]
pub(super) struct Rule {
    pub id: String,
    pub description: String,
    pub content: String,
    pub source: PathBuf,
    pub precedence: usize,
    pub always_apply: bool,
    pub matchers: Vec<String>,
}

#[derive(Clone, Debug)]
pub(super) struct Agent {
    pub id: String,
    pub source: PathBuf,
    pub precedence: usize,
    pub value: Value,
}

#[derive(Clone, Debug, Default)]
pub(super) struct Loaded {
    pub capabilities: Vec<LoadedCapability>,
    pub skills: BTreeMap<String, Skill>,
    pub rules: BTreeMap<String, Rule>,
    pub agents: BTreeMap<String, Agent>,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 80
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn collect(root: &Path, extensions: &[&str], out: &mut Vec<PathBuf>) {
    if out.len() >= MAX_DEFINITIONS || !root.exists() {
        return;
    }
    if root.is_file() {
        if root
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|extension| extensions.contains(&extension))
        {
            out.push(root.to_path_buf());
        }
        return;
    }
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    let mut paths = entries
        .flatten()
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    paths.sort();
    for path in paths {
        collect(&path, extensions, out);
        if out.len() >= MAX_DEFINITIONS {
            break;
        }
    }
}

fn parse_frontmatter(text: &str) -> Result<(Map<String, Value>, String), String> {
    let normalized = text.replace("\r\n", "\n");
    if !normalized.starts_with("---\n") {
        return Ok((Map::new(), normalized));
    }
    let tail = &normalized[4..];
    let end = tail
        .find("\n---\n")
        .ok_or("unterminated YAML frontmatter")?;
    let metadata: Value = serde_yaml::from_str(&tail[..end]).map_err(|error| error.to_string())?;
    let metadata = metadata
        .as_object()
        .cloned()
        .ok_or("frontmatter must be an object")?;
    Ok((metadata, tail[end + 5..].to_string()))
}

fn string_list(value: Option<&Value>) -> Result<Vec<String>, String> {
    match value {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::String(value)) => Ok(vec![value.clone()]),
        Some(Value::Array(values)) => values
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_string)
                    .ok_or_else(|| "list entries must be strings".to_string())
            })
            .collect(),
        Some(_) => Err("expected a string or string array".into()),
    }
}

fn validate_matchers(matchers: &[String]) -> Result<(), String> {
    for matcher in matchers {
        Regex::new(matcher).map_err(|error| format!("invalid matcher {matcher:?}: {error}"))?;
    }
    Ok(())
}

fn matches(matchers: &[String], prompt: &str) -> bool {
    matchers
        .iter()
        .any(|matcher| Regex::new(matcher).is_ok_and(|regex| regex.is_match(prompt)))
}

fn safe_assets(skill_file: &Path, raw: Option<&Value>) -> Result<Vec<PathBuf>, String> {
    let relative = string_list(raw)?;
    if relative.len() > MAX_ASSETS_PER_SKILL {
        return Err(format!("skill exceeds {MAX_ASSETS_PER_SKILL} assets"));
    }
    let root = skill_file.parent().unwrap_or_else(|| Path::new("."));
    let canonical_root = fs::canonicalize(root).map_err(|error| error.to_string())?;
    let mut assets = Vec::new();
    for asset in relative {
        let candidate = root.join(&asset);
        let canonical = fs::canonicalize(&candidate)
            .map_err(|error| format!("skill asset {asset}: {error}"))?;
        if !canonical.starts_with(&canonical_root) || !canonical.is_file() {
            return Err(format!("unsafe skill asset: {asset}"));
        }
        assets.push(canonical);
    }
    Ok(assets)
}

fn skill_file_id(path: &Path) -> String {
    let file = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("skill");
    if file.eq_ignore_ascii_case("skill") {
        path.parent()
            .and_then(Path::file_name)
            .and_then(|value| value.to_str())
            .unwrap_or(file)
            .to_string()
    } else {
        file.to_string()
    }
}

fn load_skill(path: &Path, precedence: usize) -> Result<Skill, String> {
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

fn load_rule(path: &Path, precedence: usize) -> Result<Rule, String> {
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

fn load_agent(path: &Path, precedence: usize) -> Result<Agent, String> {
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

fn capability_from_result<T>(
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

#[derive(Clone, Debug)]
pub(crate) struct PromptContribution {
    pub id: String,
    pub kind: &'static str,
    pub content: String,
    pub source: PathBuf,
    pub precedence: usize,
    pub assets: Vec<PathBuf>,
}
