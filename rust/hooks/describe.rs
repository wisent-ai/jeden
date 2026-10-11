use super::*;

/// Human summary of configured hooks (for `/hooks`), split by trust origin,
/// plus the resolved Tama registry source when one is active.
/// Only lists the events the runtime actually fires. A config that cannot be
/// read or is not JSON is listed with its cause: the hooks it declares refuse
/// every event until it is repaired.
pub fn describe_hooks(cwd: &Path) -> String {
    let project = read_config(&project_hooks_path(cwd));
    let user = match user_hooks_path() {
        Some(path) => read_config(&path),
        None => Ok(Value::Null),
    };
    let events = event::ALL;
    let mut lines = Vec::new();
    for (label, config) in [
        ("User (~/.jeden/hooks.json, always trusted)", &user),
        (
            "Project (.jeden/hooks.json, runs only with --allow-command)",
            &project,
        ),
    ] {
        let config = match config {
            Ok(config) => config,
            Err(reason) => {
                lines.push(label.to_string());
                lines.push(format!(
                    "  unreadable; wherever these hooks apply, every event is refused: {reason}"
                ));
                continue;
            }
        };
        let mut section = Vec::new();
        for event in events {
            let hooks = parse_event_hooks(config, event);
            for hook in hooks {
                let m = if hook.matcher.is_empty() {
                    "*".to_string()
                } else {
                    hook.matcher.clone()
                };
                section.push(format!("  {} [{}] {}", event, m, hook.command));
            }
        }
        if !section.is_empty() {
            lines.push(label.to_string());
            lines.extend(section);
        }
    }
    for config in crate::slash::installed_plugin_hook_configs(cwd, true) {
        if let Err(reason) = config {
            lines.push(format!(
                "Plugin hooks unreadable; wherever they apply, every event is refused: {reason}"
            ));
        }
    }
    // Tama registry section (claude-style shared catalog); absent entirely
    // when no registry is configured or it holds nothing runnable.
    if let Some(source) = tama::describe_source(cwd) {
        lines.push(source);
    }
    if lines.is_empty() {
        format!(
            "No hooks configured.\nAdd them to {} (project) or ~/.jeden/hooks.json (user).\nEvents: SessionStart, UserPromptSubmit, PreToolUse (exit 2 blocks), PostToolUse, Stop.\nProject hooks run only with --allow-command.",
            project_hooks_path(cwd).display()
        )
    } else {
        lines.join("\n")
    }
}
