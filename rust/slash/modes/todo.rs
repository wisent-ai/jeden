use crate::slash::common::{now_text, resolve_cwd_path, split_args};
use crate::slash::state::{ModeState, TodoState};
use crate::slash::SlashContext;
use std::fs;

pub(crate) fn handle_todo(args: &str, state: &mut ModeState, context: &SlashContext<'_>) -> Result<String, String> {
    let argv = split_args(args);
    let verb = argv.first().map(String::as_str).unwrap_or("list");
    let text = argv
        .split_first()
        .map(|(_, rest)| rest.iter().cloned().collect::<Vec<_>>().join(" "))
        .unwrap_or_default();
    if verb.is_empty() || verb == "list" {
        if state.todos.is_empty() { return Ok("Todo list is empty.".into()); }
        return Ok(state.todos.iter().enumerate().map(|(index, todo)| format!("{}. [{}] {}", index + usize::from(true), todo.status, todo.text)).collect::<Vec<_>>().join("\n"));
    }
    if verb == "add" || verb == "start" {
        if text.is_empty() { return Err(format!("Usage: /todo {} <task>", verb)); }
        state.todos.push(TodoState { text: text.clone(), status: if verb == "start" { "in_progress".into() } else { "pending".into() }, created_at: now_text() });
        return Ok(format!("Todo added: {}", text));
    }
    if verb == "done" || verb == "drop" || verb == "rm" {
        let needle = text.to_ascii_lowercase();
        let Some(index) = state.todos.iter().position(|todo| todo.text.to_ascii_lowercase().contains(&needle)).or_else(|| text.parse::<usize>().ok().and_then(|n| n.checked_sub(usize::from(true))).filter(|&n| n < state.todos.len())) else {
            return Err(format!("Todo not found: {}", if text.is_empty() { "(missing)" } else { &text }));
        };
        let todo_text = state.todos[index].text.clone();
        if verb == "rm" { state.todos.remove(index); }
        else { state.todos[index].status = if verb == "done" { "done".into() } else { "dropped".into() }; }
        return Ok(format!("{} todo: {}", if verb == "rm" { "Removed" } else { "Updated" }, todo_text));
    }
    if verb == "copy" || verb == "export" {
        let md = if state.todos.is_empty() { "- [ ]".into() } else { state.todos.iter().map(|todo| format!("- [{}] {}", if todo.status == "done" { "x" } else { " " }, todo.text)).collect::<Vec<_>>().join("\n") };
        if verb == "copy" { return Ok(md); }
        let target = resolve_cwd_path(context.cwd, if text.is_empty() { "TODO.md" } else { &text });
        if let Some(parent) = target.parent() { fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
        fs::write(&target, format!("{}\n", md)).map_err(|e| e.to_string())?;
        return Ok(format!("Todos exported to {}", target.display()));
    }
    if verb == "import" {
        let target = resolve_cwd_path(context.cwd, if text.is_empty() { "TODO.md" } else { &text });
        let raw = fs::read_to_string(&target).map_err(|e| e.to_string())?;
        state.todos = raw.lines().filter_map(|line| {
            let trimmed = line.trim_start();
            let after_open = trimmed.strip_prefix("- [")?;
            let status_mark = after_open.chars().next()?;
            let text = after_open.get(status_mark.len_utf8()..)?.strip_prefix("]")?.trim().to_string();
            if text.is_empty() { return None; }
            Some(TodoState { text, status: if status_mark == 'x' || status_mark == 'X' { "done".into() } else { "pending".into() }, created_at: now_text() })
        }).collect();
        return Ok(format!("Imported {} todos from {}", state.todos.len(), target.display()));
    }
    Err("Usage: /todo [list|add|start|done|drop|rm|copy|export|import]".into())
}
