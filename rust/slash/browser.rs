use serde_json::{json, Map, Value};

use super::common::{is_plain_object, merged_config, now_text, project_config_path, read_json_value, split_args, write_json_value};
use super::SlashContext;

fn browser_record_from(config: &Value) -> Value {
    let browser = config.get("browser").filter(|value| is_plain_object(value)).cloned().unwrap_or_else(|| json!({}));
    let mode = browser.get("mode").and_then(Value::as_str)
        .or_else(|| config.get("browserMode").and_then(Value::as_str))
        .filter(|mode| matches!(*mode, "headless" | "visible"))
        .unwrap_or("headless");
    json!({
        "mode": mode,
        "updatedAt": browser.get("updatedAt").and_then(Value::as_str),
        "launch": browser.get("launch").filter(|value| is_plain_object(value)).cloned().unwrap_or_else(|| json!({})),
        "profile": browser.get("profile").filter(|value| is_plain_object(value)).cloned().unwrap_or_else(|| json!({})),
    })
}

fn format_browser_settings(label: &str, value: &Value) -> String {
    if value.as_object().map(|map| map.is_empty()).unwrap_or(true) {
        format!("{label}: (none)")
    } else {
        format!("{label}: {}", value)
    }
}

fn browser_option_value(key: &str, value: &str) -> Value {
    if value == "true" { return json!(true); }
    if value == "false" { return json!(false); }
    if matches!(key, "slowMo" | "timeout") {
        if let Ok(number) = value.parse::<f64>() { return json!(number); }
    }
    if key == "args" {
        return json!(value.split(',').map(str::trim).filter(|part| !part.is_empty()).collect::<Vec<_>>());
    }
    json!(value)
}

fn insert_nested_object(target: &mut Value, path: &[&str], value: Value) {
    if !target.is_object() { *target = json!({}); }
    let Some((key, parents)) = path.split_last() else { return; };
    let mut cursor = target.as_object_mut().expect("object");
    for part in parents {
        let next = cursor.entry((*part).to_string()).or_insert_with(|| json!({}));
        if !next.is_object() { *next = json!({}); }
        cursor = next.as_object_mut().expect("nested object");
    }
    cursor.insert((*key).to_string(), value);
}

fn valid_option_key_part(part: &str) -> bool {
    let mut chars = part.chars();
    match chars.next() {
        Some(first) => first.is_ascii_alphabetic() && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'),
        None => false,
    }
}

fn parse_browser_options(tokens: &[String]) -> Result<(Value, Value), String> {
    let mut launch = json!({});
    let mut profile = json!({});
    for token in tokens {
        let Some((raw_key, raw_value)) = token.split_once('=') else {
            return Err(format!("Expected key=value option, got \"{token}\"."));
        };
        let key_parts = raw_key.split('.').filter(|part| !part.is_empty()).collect::<Vec<_>>();
        let Some((scope, rest)) = key_parts.split_first() else {
            return Err(format!("Invalid browser option key \"{raw_key}\"."));
        };
        let scope = *scope;
        if key_parts.iter().any(|part| matches!(*part, "__proto__" | "constructor" | "prototype") || !valid_option_key_part(part)) {
            return Err(format!("Invalid browser option key \"{raw_key}\"."));
        }
        if scope == "launch" && !rest.is_empty() {
            insert_nested_object(&mut launch, rest, browser_option_value(rest.last().copied().unwrap_or(""), raw_value));
        } else if scope == "profile" && !rest.is_empty() {
            insert_nested_object(&mut profile, rest, browser_option_value(rest.last().copied().unwrap_or(""), raw_value));
        } else if raw_key == "launch" {
            insert_nested_object(&mut launch, &["executablePath"], browser_option_value("executablePath", raw_value));
        } else if raw_key == "profile" {
            insert_nested_object(&mut profile, &["name"], browser_option_value("name", raw_value));
        } else if matches!(raw_key, "args" | "channel" | "devtools" | "executablePath" | "slowMo" | "timeout") {
            insert_nested_object(&mut launch, &[raw_key], browser_option_value(raw_key, raw_value));
        } else if matches!(raw_key, "name" | "profileDir" | "profile" | "userDataDir") {
            let normalized = if raw_key == "profileDir" { "userDataDir" } else { raw_key };
            insert_nested_object(&mut profile, &[normalized], browser_option_value(normalized, raw_value));
        } else {
            return Err(format!("Unknown browser option \"{raw_key}\". Use launch.<key>=value or profile.<key>=value."));
        }
    }
    Ok((launch, profile))
}

pub(super) fn handle_browser(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let argv = split_args(args);
    let verb = argv.first().map(String::as_str).unwrap_or("");
    let config = merged_config(context.cwd);
    let current = browser_record_from(&config);
    let file = project_config_path(context.cwd);
    if verb.is_empty() || verb == "status" {
        return Ok([
            format!("Browser runtime preference: {}", current.get("mode").and_then(Value::as_str).unwrap_or("headless")),
            format!("Updated: {}", current.get("updatedAt").and_then(Value::as_str).unwrap_or("not set locally")),
            format!("Config: {}", file.display()),
            format_browser_settings("Launch settings", current.get("launch").unwrap_or(&Value::Null)),
            format_browser_settings("Profile settings", current.get("profile").unwrap_or(&Value::Null)),
            "Scope: configures the browser tool/controller backend selected by local Jeden config.".into(),
        ].join("\n"));
    }
    if !matches!(verb, "headless" | "visible") {
        return Err("Usage: /browser [status|headless|visible] [launch.<key>=value] [profile.<key>=value]".into());
    }
    let rest = argv.split_first().map(|(_, rest)| rest).unwrap_or(&[]);
    let (launch, profile) = parse_browser_options(rest)
        .map_err(|error| format!("Usage: /browser [status|headless|visible] [launch.<key>=value] [profile.<key>=value]\n{error}"))?;
    let mut merged_launch = current.get("launch").cloned().unwrap_or_else(|| json!({}));
    let mut merged_profile = current.get("profile").cloned().unwrap_or_else(|| json!({}));
    if let (Some(target), Some(source)) = (merged_launch.as_object_mut(), launch.as_object()) {
        for (key, value) in source { target.insert(key.clone(), value.clone()); }
    }
    if let (Some(target), Some(source)) = (merged_profile.as_object_mut(), profile.as_object()) {
        for (key, value) in source { target.insert(key.clone(), value.clone()); }
    }
    let mut project = read_json_value(&file);
    if !project.is_object() { project = json!({}); }
    let object = project.as_object_mut().expect("project object");
    let mut browser = Map::new();
    browser.insert("mode".into(), json!(verb));
    browser.insert("updatedAt".into(), json!(now_text()));
    if merged_launch.as_object().map(|map| !map.is_empty()).unwrap_or(false) { browser.insert("launch".into(), merged_launch.clone()); }
    if merged_profile.as_object().map(|map| !map.is_empty()).unwrap_or(false) { browser.insert("profile".into(), merged_profile.clone()); }
    object.insert("browser".into(), Value::Object(browser));
    object.remove("browserMode");
    write_json_value(&file, &project)?;
    let mut lines = vec![format!("Browser runtime preference set to {verb}."), format!("Config: {}", file.display())];
    if merged_launch.as_object().map(|map| !map.is_empty()).unwrap_or(false) { lines.push(format_browser_settings("Launch settings", &merged_launch)); }
    if merged_profile.as_object().map(|map| !map.is_empty()).unwrap_or(false) { lines.push(format_browser_settings("Profile settings", &merged_profile)); }
    lines.push("Honest scope: this configures Jeden browser-tool/controller preference only; browser availability still depends on installed local tools or MCP adapters.".into());
    Ok(lines.join("\n"))
}
