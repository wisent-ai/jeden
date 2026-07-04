use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
}

impl ToolInfo {
    fn new(name: &str, description: &str) -> Self {
        Self { name: name.to_string(), description: description.to_string() }
    }
}

fn dirs_home() -> PathBuf {
    env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

fn built_in_tools() -> Vec<ToolInfo> {
    vec![
        ToolInfo::new("list_dir", "List a directory under cwd; optional depth recursively includes children"),
        ToolInfo::new("read_file", "Read a UTF-8 text file under cwd, capped at 512KB unless a selector narrows output; returns sha256, snapshot path#TAG, visual numbered lines, content; selectors support ranges, comma ranges, raw, and conflicts"),
        ToolInfo::new("read_binary_file", "Read one binary file under cwd as base64, capped at 512KB"),
        ToolInfo::new("read_image", "Read one PNG, JPEG, GIF, or WebP image under cwd as base64 with mime type and dimensions, capped at 512KB"),
        ToolInfo::new("read_archive", "List archive entries or read one entry from .zip, .tar, .tar.gz, or .tgz under cwd as text, binary base64, document text, or image metadata; text/document entries support line ranges"),
        ToolInfo::new("read_document", "Extract readable text from one document under cwd; supports text, HTML, JSON, CSV/TSV, XML/RSS/Atom, notebooks, and basic PDF text streams; supports line ranges; capped at 512KB output"),
        ToolInfo::new("read_sqlite", "Read a SQLite database under cwd: list tables, inspect a table, fetch one row by primary key, or run a read-only SELECT/WITH query"),
        ToolInfo::new("search_text", "Search one file for a literal string, capped at 50 line matches; case-insensitive by default"),
        ToolInfo::new("search_files", "Recursively search text files under cwd for a literal string, capped at 500 matches; supports path/paths, hidden, gitignore, caseSensitive, limit, and skip; case-insensitive by default"),
        ToolInfo::new("glob_paths", "Find files and directories under cwd with simple glob patterns; supports * and ** plus hidden, gitignore, limit, and skip"),
        ToolInfo::new("grep_regex", "Search text files under cwd with a JavaScript regular expression, capped at 500 matches; supports path/paths, hidden, gitignore, multiline, caseSensitive, limit, and skip; case-insensitive by default"),
        ToolInfo::new("write_file", "Create or overwrite a UTF-8 text file under cwd; overwrites require expectedSha256 from read_file; returns sha256 and visual diff; requires --allow-write"),
        ToolInfo::new("apply_patch", "Apply exact one-occurrence string replacements to an existing UTF-8 file; returns sha256 and visual diff; requires expectedSha256 and --allow-write"),
        ToolInfo::new("edit", "Apply an OMP-style anchored visual patch string with [path#TAG], SWAP/DEL/INS/REM/MV and safe block hunks, visual diffs, and tag freshness checks; requires --allow-write"),
        ToolInfo::new("edit_file", "Apply line-based edits to a UTF-8 file under cwd; returns sha256 and visual diff; requires expectedSha256 and --allow-write"),
        ToolInfo::new("delete_file", "Delete one UTF-8 file under cwd; returns visual diff; requires expectedSha256 and --allow-write"),
        ToolInfo::new("move_file", "Move or rename one file under cwd; returns rename preview; requires expectedSha256 and --allow-write"),
        ToolInfo::new("run_command", "Run a shell command in cwd; requires --allow-command; supports env overrides; timeout defaults to 30s and maxes at 120s"),
        ToolInfo::new("run_process", "Run one process with argv array in cwd without a shell; requires --allow-command; supports env overrides"),
        ToolInfo::new("node_eval", "Run JavaScript with node --input-type=module in cwd; requires --allow-command"),
        ToolInfo::new("python_eval", "Run Python code with python3 in cwd; requires --allow-command"),
        ToolInfo::new("list_package_scripts", "List package.json scripts in cwd"),
        ToolInfo::new("run_package_script", "Run one existing package.json script with npm; requires --allow-command or interactive approval; supports env overrides"),
        ToolInfo::new("git_status", "Read git status --short for cwd"),
        ToolInfo::new("git_diff", "Read git diff for cwd or one path under cwd"),
        ToolInfo::new("git_log", "Read recent git commits for cwd or one path under cwd"),
        ToolInfo::new("git_show", "Read one git object or commit summary; optional path scopes the output"),
        ToolInfo::new("fetch_url", "Fetch one HTTP(S) URL and return text capped at maxBytes with byte count, truncation state, SHA-256, optional timeoutMs, and optional line range"),
        ToolInfo::new("fetch_readable_url", "Fetch one HTTP(S) URL and return simplified readable text capped at maxBytes with byte count, truncation state, SHA-256, optional timeoutMs, and optional line range; supports HTML, JSON, CSV/TSV, RSS/Atom/XML, notebooks, and basic PDF text streams"),
        ToolInfo::new("save_artifact", "Save UTF-8 content into the current session artifacts directory"),
        ToolInfo::new("list_artifacts", "List files in the current session artifact directory"),
        ToolInfo::new("read_artifact", "Read one UTF-8 artifact from the current session artifact directory"),
        ToolInfo::new("ask_user", "Ask the human user a question during an interactive session"),
        ToolInfo::new("todo", "Manage the current session todo list with init, append, start, done, drop, rm, and view operations; supports phased lists"),
        ToolInfo::new("delegate_task", "Run a focused subtask in a fresh Jeden session and return its result"),
        ToolInfo::new("memory", "Remember and recall durable scoped notes across Jeden sessions"),
        ToolInfo::new("mcp_list_tools", "List tools from a configured stdio MCP server"),
        ToolInfo::new("mcp_call_tool", "Call one tool on a configured stdio MCP server"),
        ToolInfo::new("mcp_list_resources", "List resources from a configured stdio MCP server"),
        ToolInfo::new("mcp_read_resource", "Read one resource from a configured stdio MCP server"),
        ToolInfo::new("mcp_list_prompts", "List prompts from a configured stdio MCP server"),
        ToolInfo::new("mcp_get_prompt", "Get one prompt from a configured stdio MCP server"),
    ]
}

fn tool_dirs(cwd: &Path) -> Vec<PathBuf> {
    let project = cwd.join(".jeden/tools");
    let home = dirs_home().join(".jeden/tools");
    if home == project { vec![home] } else { vec![home, project] }
}

fn read_json_value(path: &Path) -> Value {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .unwrap_or(Value::Null)
}

fn js_string_after(source: &str, key: &str, from: usize) -> Option<(String, usize)> {
    let needle = format!("{}:", key);
    let key_pos = source[from..].find(&needle)? + from + needle.len();
    let rest = &source[key_pos..];
    let quote_rel = rest.find(|c| c == '\'' || c == '"')?;
    let quote_pos = key_pos + quote_rel;
    let quote = source.as_bytes()[quote_pos] as char;
    let mut out = String::new();
    let mut escaped = false;
    for (offset, ch) in source[quote_pos + 1..].char_indices() {
        if escaped {
            out.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == quote { return Some((out, quote_pos + 1 + offset + ch.len_utf8())); }
        out.push(ch);
    }
    None
}

fn static_custom_tools(cwd: &Path, seen: &mut BTreeSet<String>) -> Vec<ToolInfo> {
    let mut tools = Vec::new();
    for dir in tool_dirs(cwd) {
        let Ok(entries) = fs::read_dir(dir) else { continue };
        let mut files = entries.flatten().map(|entry| entry.path()).collect::<Vec<_>>();
        files.sort();
        for file in files {
            let Some(ext) = file.extension().and_then(|v| v.to_str()) else { continue };
            if ext != "js" && ext != "mjs" { continue; }
            let Ok(source) = fs::read_to_string(&file) else { continue };
            let mut cursor = 0;
            while let Some((name, next)) = js_string_after(&source, "name", cursor) {
                cursor = next;
                if seen.contains(&name) { continue; }
                let description = js_string_after(&source, "description", cursor)
                    .map(|(value, _)| value)
                    .unwrap_or_else(|| format!("Custom tool from {}", file.display()));
                seen.insert(name.clone());
                tools.push(ToolInfo { name, description });
            }
        }
    }
    tools
}

fn native_mcp_tool_name(server_name: &str, tool_name: &str) -> String {
    fn safe(value: &str) -> String {
        value.chars().map(|ch| if ch.is_ascii_alphanumeric() || ch == '_' { ch } else { '_' }).collect()
    }
    format!("mcp__{}__{}", safe(server_name), safe(tool_name))
}

fn merge_mcp_config(cwd: &Path) -> Value {
    let user = read_json_value(&dirs_home().join(".jeden/mcp.json"));
    let project = read_json_value(&cwd.join(".jeden/mcp.json"));
    let mut servers = serde_json::Map::new();
    for source in [&user, &project] {
        if let Some(map) = source.get("mcpServers").and_then(Value::as_object) {
            for (name, server) in map { servers.insert(name.clone(), server.clone()); }
        }
    }
    let mut disabled = Vec::new();
    for source in [&user, &project] {
        if let Some(values) = source.get("disabledServers").and_then(Value::as_array) {
            disabled.extend(values.iter().filter_map(Value::as_str).map(ToString::to_string));
        }
    }
    serde_json::json!({"mcpServers": servers, "disabledServers": disabled})
}

pub fn configured_mcp_server_names(cwd: &Path) -> Vec<String> {
    crate::mcp::configured_server_names(cwd)
}

fn static_mcp_tools(cwd: &Path, seen: &mut BTreeSet<String>) -> Vec<ToolInfo> {
    let config = merge_mcp_config(cwd);
    let disabled = config.get("disabledServers").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect::<BTreeSet<_>>();
    let mut out = Vec::new();
    let Some(servers) = config.get("mcpServers").and_then(Value::as_object) else { return out };
    let ordered: BTreeMap<_, _> = servers.iter().collect();
    for (server_name, server) in ordered {
        if disabled.contains(server_name.as_str()) { continue; }
        let Some(tools) = server.get("tools").and_then(Value::as_array) else { continue; };
        for tool in tools {
            let Some(raw_name) = tool.get("name").and_then(Value::as_str) else { continue; };
            let native_name = native_mcp_tool_name(server_name, raw_name);
            if seen.contains(&native_name) { continue; }
            let description = tool.get("description").and_then(Value::as_str).map(ToString::to_string).unwrap_or_else(|| format!("MCP tool {} from {}", raw_name, server_name));
            seen.insert(native_name.clone());
            out.push(ToolInfo { name: native_name, description });
        }
    }
    out
}

pub fn list_tools(cwd: &Path) -> Vec<ToolInfo> {
    let mut seen = BTreeSet::new();
    let mut tools = built_in_tools();
    for tool in &tools { seen.insert(tool.name.clone()); }
    tools.extend(static_custom_tools(cwd, &mut seen));
    tools.extend(static_mcp_tools(cwd, &mut seen));
    tools
}

pub fn tools_table(cwd: &Path) -> String {
    let mut out = String::new();
    for tool in list_tools(cwd) {
        out.push_str(&tool.name);
        out.push('\t');
        out.push_str(&tool.description);
        out.push('\n');
    }
    out
}

pub fn tools_slash_text(cwd: &Path) -> String {
    let mut lines = vec!["Tools visible to Jeden:".to_string()];
    lines.extend(list_tools(cwd).into_iter().map(|tool| format!("- {}: {}", tool.name, tool.description)));
    lines.join("\n")
}
