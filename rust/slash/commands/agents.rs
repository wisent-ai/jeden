use crate::slash::common::split_head;
use crate::slash::SlashContext;
use crate::task_runtime::AgentDefinition;
use crate::tui::{PickerItem, PickerSpec};

/// `JEDEN_AGENT_TOOLS` semantics: an empty tool list admits every tool.
fn tools_label(definition: &AgentDefinition) -> String {
    if definition.tools.is_empty() {
        "all tools".to_string()
    } else {
        format!("{} tools", definition.tools.len())
    }
}

pub(crate) fn agents_picker(context: &SlashContext<'_>) -> PickerSpec {
    let definitions = match crate::task_runtime::discover_agents(context.cwd) {
        Ok(definitions) => definitions,
        Err(error) => {
            return PickerSpec::new(
                "Agents",
                vec![PickerItem::action("Agent discovery failed", "")
                    .detail(error.to_string())
                    .badge("error")
                    .disabled(true)],
            );
        }
    };
    let mut items: Vec<PickerItem> = definitions
        .iter()
        .map(|definition| {
            let description = if definition.description.trim().is_empty() {
                definition.source.display().to_string()
            } else {
                definition.description.clone()
            };
            PickerItem::action(
                definition.id.clone(),
                format!("/agents show {}", definition.id),
            )
            .detail(format!(
                "{} · {} — Enter shows the full definition",
                tools_label(definition),
                description
            ))
            .badge(if definition.id == "default" {
                "DEFAULT"
            } else {
                "CUSTOM"
            })
        })
        .collect();
    items.push(
        PickerItem::action("Jobs", "/jobs")
            .detail("Open the background jobs view for locally tracked agent jobs")
            .badge("view"),
    );
    PickerSpec::new("Agents", items)
}

pub(crate) fn handle_agents(args: &str, context: &SlashContext<'_>) -> Result<String, String> {
    let (verb, rest) = split_head(args);
    if verb.is_empty() {
        return Ok("Agent controls:\n- /agents opens the agent picker.\n- /agents show <name> prints one agent definition.\n- /tan <work> starts a detached local agent job tracked in session artifacts.\n- /advisor manages second-pass reviewer mode.\n- /jobs shows locally tracked background jobs.".into());
    }
    if verb != "show" {
        return Err("Usage: /agents [show <name>]".into());
    }
    let name = rest.trim();
    if name.is_empty() {
        return Err("Usage: /agents show <name>".into());
    }
    let definitions =
        crate::task_runtime::discover_agents(context.cwd).map_err(|error| error.to_string())?;
    let definition = definitions
        .iter()
        .find(|definition| definition.id == name)
        .ok_or_else(|| format!("Unknown agent: {name}"))?;
    let body = serde_json::to_string_pretty(definition).map_err(|error| error.to_string())?;
    Ok(format!("source: {}\n{}", definition.source.display(), body))
}
