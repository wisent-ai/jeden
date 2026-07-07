use serde_json::{json, Value};
use std::fs;

use crate::tool_runtime::shared::string_input;
use crate::tool_runtime::ToolRuntime;

fn todo_item(value: &Value) -> Value {
    let text = value.as_str()
        .or_else(|| value.get("text").and_then(Value::as_str))
        .or_else(|| value.get("task").and_then(Value::as_str))
        .or_else(|| value.get("name").and_then(Value::as_str))
        .unwrap_or("");
    let status = value.get("status").and_then(Value::as_str).unwrap_or("pending");
    json!({"text": text, "status": status})
}

fn todo_summary(state: &mut Value) -> Value {
    let mut items_flat = Vec::new();
    let mut has_active = false;
    if let Some(phases) = state.get_mut("phases").and_then(Value::as_array_mut) {
        for phase in phases.iter_mut() {
            if let Some(items) = phase.get_mut("items").and_then(Value::as_array_mut) {
                if items.iter().any(|item| item.get("status").and_then(Value::as_str) == Some("in_progress")) {
                    has_active = true;
                }
            }
        }
        if !has_active {
            'outer: for phase in phases.iter_mut() {
                if let Some(items) = phase.get_mut("items").and_then(Value::as_array_mut) {
                    for item in items {
                        if item.get("status").and_then(Value::as_str) == Some("pending") {
                            item["status"] = json!("in_progress");
                            break 'outer;
                        }
                    }
                }
            }
        }
        for phase in phases.iter() {
            let phase_name = phase.get("phase").and_then(Value::as_str).unwrap_or("Tasks");
            if let Some(items) = phase.get("items").and_then(Value::as_array) {
                for item in items {
                    let mut flat = item.clone();
                    flat["phase"] = json!(phase_name);
                    items_flat.push(flat);
                }
            }
        }
    }
    let completed = items_flat.iter().filter(|item| item.get("status").and_then(Value::as_str) == Some("done")).count();
    let active = items_flat.iter()
        .find(|item| item.get("status").and_then(Value::as_str) == Some("in_progress"))
        .or_else(|| items_flat.iter().find(|item| !matches!(item.get("status").and_then(Value::as_str), Some("done" | "dropped"))))
        .and_then(|item| item.get("text").and_then(Value::as_str))
        .map(ToString::to_string);
    json!({"total": items_flat.len(), "completed": completed, "active": active, "phases": state.get("phases").cloned().unwrap_or_else(|| json!([])), "items": items_flat})
}

pub(crate) fn todo_tool(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let Some(dir) = runtime.artifact_dir else { return Err("todo requires an active session".into()); };
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let file = dir.join("todo.json");
    let mut state: Value = fs::read_to_string(&file).ok().and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or_else(|| json!({"phases": []}));
    let op = string_input(input, "op").unwrap_or_else(|| "view".into());
    if op == "init" {
        if let Some(list) = input.get("list").and_then(Value::as_array) {
            let phases = list.iter().map(|phase| {
                let name = phase.get("phase").or_else(|| phase.get("name")).and_then(Value::as_str).unwrap_or("Tasks");
                let items = phase.get("items").and_then(Value::as_array).cloned().unwrap_or_default().iter().map(todo_item).collect::<Vec<_>>();
                json!({"phase": name, "items": items})
            }).collect::<Vec<_>>();
            state["phases"] = json!(phases);
        } else {
            let name = string_input(input, "phase").unwrap_or_else(|| "Tasks".into());
            let items = input.get("items").and_then(Value::as_array).cloned().unwrap_or_default().iter().map(todo_item).collect::<Vec<_>>();
            state["phases"] = json!([{"phase": name, "items": items}]);
        }
    } else if op == "append" {
        let name = string_input(input, "phase").unwrap_or_else(|| "Tasks".into());
        let new_items = input.get("items").and_then(Value::as_array).ok_or("items are required")?;
        let phases = state["phases"].as_array_mut().ok_or("invalid todo state")?;
        let idx = phases.iter().position(|phase| phase.get("phase").and_then(Value::as_str) == Some(name.as_str())).unwrap_or_else(|| {
            phases.push(json!({"phase": name, "items": []}));
            phases.len() - 1
        });
        let items = phases[idx]["items"].as_array_mut().ok_or("invalid todo phase")?;
        items.extend(new_items.iter().map(todo_item));
    } else if matches!(op.as_str(), "start" | "done" | "drop") {
        let status = if op == "start" { "in_progress" } else if op == "done" { "done" } else { "dropped" };
        let mut found = false;
        if let Some(phase_name) = string_input(input, "phase").filter(|_| op != "start" && string_input(input, "task").is_none()) {
            if let Some(phases) = state["phases"].as_array_mut() {
                for phase in phases {
                    if phase.get("phase").and_then(Value::as_str) == Some(phase_name.as_str()) {
                        if let Some(items) = phase["items"].as_array_mut() {
                            for item in items { item["status"] = json!(status); }
                            found = true;
                        }
                    }
                }
            }
            if !found { return Err(format!("unknown phase: {phase_name}")); }
        } else {
            let task = string_input(input, "task").ok_or("task is required")?;
            if let Some(phases) = state["phases"].as_array_mut() {
                for phase in phases {
                    if let Some(items) = phase["items"].as_array_mut() {
                        for item in items {
                            if op == "start" && item.get("status").and_then(Value::as_str) == Some("in_progress") { item["status"] = json!("pending"); }
                            if item.get("text").and_then(Value::as_str) == Some(task.as_str()) { item["status"] = json!(status); found = true; }
                        }
                    }
                }
            }
            if !found { return Err(format!("unknown task: {task}")); }
        }
    } else if op == "rm" {
        if let Some(phase_name) = string_input(input, "phase") {
            if let Some(phases) = state["phases"].as_array_mut() {
                phases.retain(|phase| phase.get("phase").and_then(Value::as_str) != Some(phase_name.as_str()));
            }
        } else if let Some(task) = string_input(input, "task") {
            if let Some(phases) = state["phases"].as_array_mut() {
                for phase in phases {
                    if let Some(items) = phase["items"].as_array_mut() { items.retain(|item| item.get("text").and_then(Value::as_str) != Some(task.as_str())); }
                }
            }
        } else {
            state["phases"] = json!([]);
        }
    } else if op != "view" {
        return Err(format!("unknown todo op: {op}"));
    }
    let summary = todo_summary(&mut state);
    fs::write(&file, serde_json::to_vec_pretty(&state).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    Ok(summary)
}
