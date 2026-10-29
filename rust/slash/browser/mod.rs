use serde_json::{json, Map, Value};

use super::common::{
    now_text, project_config_for_edit, project_config_path, split_args, write_json_value,
};
use super::SlashContext;
use crate::cli::config::merged_config_value;
use crate::tool_services::browser::{HEADLESS, VISIBLE};
use crate::tui::{PickerItem, PickerSpec};

mod options;

use options::parse_browser_options;

/// The browser preference as configuration stores it. A table no layer
/// stores is empty; one that is stored but is not an object is refused by
/// [`browser_record_from`], so neither a status nor a write reads it as
/// empty, and a write never replaces what it could not read.
struct BrowserRecord {
    /// The mode as it is stored, or none: a value /browser never stores is
    /// shown as it is, and the browser service refuses it by name, rather
    /// than being shown and run as headless.
    mode: Option<String>,
    updated_at: Option<String>,
    launch: Map<String, Value>,
    profile: Map<String, Value>,
}

/// The object stored at `key` of `table`, empty when nothing is stored
/// there; any other value is refused naming where it is.
fn stored_table(table: &Value, key: &str, place: &str) -> Result<Map<String, Value>, String> {
    match table.get(key) {
        None | Some(Value::Null) => Ok(Map::new()),
        Some(Value::Object(stored)) => Ok(stored.clone()),
        Some(other) => Err(format!(
            "{place}.{key} is {other}, not an object of settings"
        )),
    }
}

fn browser_record_from(config: &Value) -> Result<BrowserRecord, String> {
    let browser = Value::Object(stored_table(config, "browser", "configuration")?);
    let mode = browser
        .get("mode")
        .and_then(Value::as_str)
        .or_else(|| config.get("browserMode").and_then(Value::as_str))
        .map(str::to_string);
    Ok(BrowserRecord {
        mode,
        updated_at: browser
            .get("updatedAt")
            .and_then(Value::as_str)
            .map(str::to_string),
        launch: stored_table(&browser, "launch", "browser")?,
        profile: stored_table(&browser, "profile", "browser")?,
    })
}

/// The stored mode as a person reads it: no stored mode runs headless.
fn mode_text(record: &BrowserRecord) -> String {
    match &record.mode {
        Some(mode) => mode.clone(),
        None => format!("{HEADLESS} (no mode stored)"),
    }
}

fn format_browser_settings(label: &str, settings: &Map<String, Value>) -> String {
    if settings.is_empty() {
        format!("{label}: (none)")
    } else {
        format!("{label}: {}", Value::Object(settings.clone()))
    }
}

fn setting_keys(settings: &Map<String, Value>) -> String {
    if settings.is_empty() {
        "none".to_string()
    } else {
        settings.keys().cloned().collect::<Vec<_>>().join(", ")
    }
}

pub(super) fn browser_picker(context: &SlashContext<'_>) -> PickerSpec {
    let current = match browser_record_from(&merged_config_value(context.cwd)) {
        Ok(current) => current,
        Err(error) => {
            return PickerSpec::new(
                "Browser runtime",
                vec![
                    PickerItem::action("Show browser runtime status", "/browser status")
                        .detail(format!(
                            "The stored browser preference cannot be read: {error}"
                        ))
                        .badge("unreadable"),
                ],
            )
        }
    };
    let mode = mode_text(&current);
    let stored = current.mode.as_deref();
    // No stored mode runs headless, so headless is then the current one.
    let headless = stored.is_none() || stored == Some(HEADLESS);
    let visible = stored == Some(VISIBLE);
    let scope = format!("{mode} · project preference");
    let mut items = vec![
        PickerItem::action("Show browser runtime status", "/browser status")
            .detail(format!(
                "Launch keys: {}; profile keys: {}",
                setting_keys(&current.launch),
                setting_keys(&current.profile)
            ))
            .badge(scope.clone()),
        PickerItem::action("Use headless browser mode", "/browser headless")
            .detail(format!(
                "Persist preference in {}",
                project_config_path(context.cwd).display()
            ))
            .badge(if headless { "current" } else { "available" })
            .disabled(headless),
        PickerItem::action("Use visible browser mode", "/browser visible")
            .detail(format!(
                "Persist preference in {}",
                project_config_path(context.cwd).display()
            ))
            .badge(if visible { "current" } else { "available" })
            .disabled(visible),
    ];
    let options = PickerItem::action("Configure launch or profile options", "/browser visible ")
        .detail("Edit mode and launch.<key>=value or profile.<key>=value before submitting")
        .badge("INPUT")
        .prefill();
    items.push(options);
    PickerSpec::new("Browser runtime", items)
}

const USAGE: &str =
    "Usage: /browser [status|headless|visible] [launch.<key>=value] [profile.<key>=value]";

/// `/browser` with no word shows the status, as `/browser status` does.
pub(super) fn handle_browser(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let argv = split_args(args);
    let config = merged_config_value(context.cwd);
    let current = browser_record_from(&config)?;
    let file = project_config_path(context.cwd);
    let (verb, rest) = match argv.split_first() {
        None => ("status", &argv[..]),
        Some((verb, rest)) => (verb.as_str(), rest),
    };
    if verb == "status" {
        let updated = match &current.updated_at {
            Some(updated) => updated.clone(),
            None => "not set locally".to_string(),
        };
        return Ok([
            format!("Browser runtime preference: {}", mode_text(&current)),
            format!("Updated: {updated}"),
            format!("Config: {}", file.display()),
            format_browser_settings("Launch settings", &current.launch),
            format_browser_settings("Profile settings", &current.profile),
            "Scope: configures the browser tool/controller backend selected by local Jeden config."
                .into(),
        ]
        .join("\n"));
    }
    if verb != HEADLESS && verb != VISIBLE {
        return Err(USAGE.into());
    }
    let (launch, profile) =
        parse_browser_options(rest).map_err(|error| format!("{USAGE}\n{error}"))?;
    let mut merged_launch = current.launch;
    let mut merged_profile = current.profile;
    if let Some(source) = launch.as_object() {
        merged_launch.extend(source.clone());
    }
    if let Some(source) = profile.as_object() {
        merged_profile.extend(source.clone());
    }
    let mut project = project_config_for_edit(context.cwd)?;
    let object = project.as_object_mut().ok_or_else(|| {
        format!(
            "{} does not hold a JSON object, so the preference is not written",
            file.display()
        )
    })?;
    let mut browser = Map::new();
    browser.insert("mode".into(), json!(verb));
    browser.insert("updatedAt".into(), json!(now_text()?));
    if !merged_launch.is_empty() {
        browser.insert("launch".into(), Value::Object(merged_launch.clone()));
    }
    if !merged_profile.is_empty() {
        browser.insert("profile".into(), Value::Object(merged_profile.clone()));
    }
    object.insert("browser".into(), Value::Object(browser));
    object.remove("browserMode");
    write_json_value(&file, &project)?;
    let mut lines = vec![
        format!("Browser runtime preference set to {verb}."),
        format!("Config: {}", file.display()),
    ];
    if !merged_launch.is_empty() {
        lines.push(format_browser_settings("Launch settings", &merged_launch));
    }
    if !merged_profile.is_empty() {
        lines.push(format_browser_settings("Profile settings", &merged_profile));
    }
    lines.push("Honest scope: this configures Jeden browser-tool/controller preference only; browser availability still depends on installed local tools or MCP adapters.".into());
    Ok(lines.join("\n"))
}
