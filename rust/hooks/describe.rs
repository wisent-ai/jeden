use super::*;


/// Human summary of configured hooks (for `/hooks`), split by trust origin,
/// plus the resolved Tama registry source when one is active.
/// Only lists the events the runtime actually fires.
pub fn describe_hooks(cwd: &Path) -> String {
    let project = read_config(&project_hooks_path(cwd));
    let user = user_hooks_path()
        .map(|p| read_config(&p))
        .unwrap_or(Value::Null);
    let events = event::ALL;
    let mut lines = Vec::new();
    for (label, config) in [
        ("User (~/.jeden/hooks.json, always trusted)", &user),
        (
            "Project (.jeden/hooks.json, runs only with --allow-command)",
            &project,
        ),
    ] {
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
