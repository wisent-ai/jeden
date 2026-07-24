use super::*;
use crate::cli::config::{ui_language, UiLanguage};

pub(in crate::agent) fn rust_tool_specs(cwd: &Path) -> Vec<Value> {
    let mut specs = vec![
        tool_spec("list_dir", "List a directory under cwd", json!({"path": {"type": "string"}, "limit": {"type": "number"}}), vec![]),
        tool_spec("read_file", "Read a UTF-8 file under cwd", json!({"path": {"type": "string"}}), vec!["path"]),
        tool_spec("read_binary_file", "Read one binary file under cwd as base64", json!({"path": {"type": "string"}, "maxBytes": {"type": "number"}}), vec!["path"]),
        tool_spec("read_document", "Extract readable text from one document under cwd with optional line range", json!({"path": {"type": "string"}, "maxBytes": {"type": "number"}, "range": {"type": "string"}}), vec!["path"]),
        tool_spec("read_archive", "List archive entries or read one entry from .zip, .tar, .tar.gz, or .tgz under cwd", json!({"path": {"type": "string"}, "entry": {"type": "string"}, "mode": {"type": "string"}, "maxBytes": {"type": "number"}, "range": {"type": "string"}}), vec!["path"]),
        tool_spec("read_image", "Read one PNG, JPEG, GIF, or WebP image under cwd as base64 with mime type and dimensions", json!({"path": {"type": "string"}, "maxBytes": {"type": "number"}}), vec!["path"]),
        tool_spec("read_sqlite", "Read a SQLite database under cwd: list tables, inspect a table, fetch one row, or run a read-only SELECT/WITH query", json!({"path": {"type": "string"}, "table": {"type": "string"}, "key": {"type": "string"}, "query": {"type": "string"}, "limit": {"type": "number"}, "offset": {"type": "number"}, "where": {"type": "string"}, "order": {"type": "string"}}), vec!["path"]),
        tool_spec("search_text", "Search one file for a literal string", json!({"path": {"type": "string"}, "query": {"type": "string"}, "caseSensitive": {"type": "boolean"}}), vec!["path", "query"]),
        tool_spec("search_files", "Recursively search text files under cwd for a literal string", json!({"path": {"type": "string"}, "paths": {"type": "array", "items": {"type": "string"}}, "query": {"type": "string"}, "hidden": {"type": "boolean"}, "gitignore": {"type": "boolean"}, "caseSensitive": {"type": "boolean"}, "limit": {"type": "number"}, "skip": {"type": "number"}}), vec!["query"]),
        tool_spec("glob_paths", "Find files under cwd with simple glob patterns", json!({"patterns": {"type": "string"}, "path": {"type": "string"}, "hidden": {"type": "boolean"}, "gitignore": {"type": "boolean"}, "limit": {"type": "number"}, "skip": {"type": "number"}}), vec![]),
        tool_spec("grep_regex", "Search text files under cwd with a regular expression", json!({"expr": {"type": "string"}, "path": {"type": "string"}, "paths": {"type": "array", "items": {"type": "string"}}, "hidden": {"type": "boolean"}, "gitignore": {"type": "boolean"}, "multiline": {"type": "boolean"}, "caseSensitive": {"type": "boolean"}, "limit": {"type": "number"}, "skip": {"type": "number"}}), vec!["expr"]),
        tool_spec("write_file", "Create or overwrite a UTF-8 file under cwd; overwrites require expectedSha256 and write-tier approval", json!({"path": {"type": "string"}, "content": {"type": "string"}, "expectedSha256": {"type": "string"}}), vec!["path", "content"]),
        tool_spec("apply_patch", "Apply exact one-occurrence string replacements to an existing UTF-8 file; requires expectedSha256 and write-tier approval", json!({"path": {"type": "string"}, "expectedSha256": {"type": "string"}, "replacements": {"type": "array"}}), vec!["path", "expectedSha256", "replacements"]),
        tool_spec("edit_file", "Apply line-based edits to a UTF-8 file under cwd; requires expectedSha256 and write-tier approval", json!({"path": {"type": "string"}, "expectedSha256": {"type": "string"}, "ops": {"type": "array"}}), vec!["path", "expectedSha256", "ops"]),
        tool_spec("edit", "Apply a Jeden anchored visual patch string with [path#TAG], SWAP/DEL/INS/REM/MV and safe block hunks; requires write-tier approval", json!({"patch": {"type": "string"}}), vec!["patch"]),
        tool_spec("delete_file", "Delete one file under cwd; requires expectedSha256 and write-tier approval", json!({"path": {"type": "string"}, "expectedSha256": {"type": "string"}}), vec!["path", "expectedSha256"]),
        tool_spec("move_file", "Move or rename one file under cwd; requires expectedSha256 and write-tier approval", json!({"from": {"type": "string"}, "to": {"type": "string"}, "expectedSha256": {"type": "string"}, "overwrite": {"type": "boolean"}}), vec!["from", "to", "expectedSha256"]),
        tool_spec("run_command", "Run a shell command in cwd; requires exec-tier approval", json!({"command": {"type": "string"}, "timeoutMs": {"type": "number"}}), vec!["command"]),
        tool_spec("run_process", "Run one process with argv array in cwd; requires exec-tier approval", json!({"command": {"type": "string"}, "args": {"type": "array", "items": {"type": "string"}}, "stdin": {"type": "string"}, "timeoutMs": {"type": "number"}, "env": {"type": "object"}}), vec!["command"]),
        tool_spec("node_eval", "Run JavaScript with node --input-type=module in cwd; requires exec-tier approval", json!({"code": {"type": "string"}, "timeoutMs": {"type": "number"}}), vec!["code"]),
        tool_spec("python_eval", "Run Python code with python3 in cwd; requires exec-tier approval", json!({"code": {"type": "string"}, "timeoutMs": {"type": "number"}}), vec!["code"]),
        tool_spec("list_package_scripts", "List package.json scripts in cwd", json!({}), vec![]),
        tool_spec("run_package_script", "Run one existing package.json script with npm; requires exec-tier approval", json!({"script": {"type": "string"}, "timeoutMs": {"type": "number"}, "env": {"type": "object"}}), vec!["script"]),
        tool_spec("git_status", "Read git status --short for cwd", json!({}), vec![]),
        tool_spec("git_diff", "Read git diff for cwd or one path under cwd", json!({"path": {"type": "string"}}), vec![]),
        tool_spec("git_log", "Read recent git commits for cwd or one path under cwd", json!({"limit": {"type": "number"}, "path": {"type": "string"}}), vec![]),
        tool_spec("git_show", "Read one git object or commit summary", json!({"ref": {"type": "string"}, "path": {"type": "string"}}), vec![]),
        tool_spec("fetch_url", "Fetch one HTTP(S) URL and return capped text; supports optional line range", json!({"url": {"type": "string"}, "maxBytes": {"type": "number"}, "timeoutMs": {"type": "number"}, "range": {"type": "string"}}), vec!["url"]),
        tool_spec("fetch_readable_url", "Fetch one HTTP(S) URL and return simplified readable text with optional line range", json!({"url": {"type": "string"}, "maxBytes": {"type": "number"}, "timeoutMs": {"type": "number"}, "range": {"type": "string"}}), vec!["url"]),
        tool_spec("save_artifact", "Save UTF-8 content into the current session artifacts directory", json!({"name": {"type": "string"}, "content": {"type": "string"}}), vec!["content"]),
        tool_spec("list_artifacts", "List files in the current session artifact directory", json!({}), vec![]),
        tool_spec("read_artifact", "Read one UTF-8 artifact from the current session artifact directory", json!({"name": {"type": "string"}, "maxBytes": {"type": "number"}}), vec!["name"]),
        tool_spec("recall_conversation", "Return the text-only transcript of a recorded session (user prompts and final answers; tool calls, results, and images stripped); defaults to the current session, or pass session=<id-or-path>", json!({"session": {"type": "string"}}), vec![]),
        tool_spec("ask_user", "Ask the human user a question during an interactive session", json!({"question": {"type": "string"}, "options": {"type": "array"}}), vec!["question"]),
        tool_spec("todo", "Manage the current session todo list with init, append, start, done, drop, rm, and view operations", json!({"op": {"type": "string"}, "list": {"type": "array"}, "phase": {"type": "string"}, "items": {"type": "array"}, "task": {"type": "string"}}), vec!["op"]),
        tool_spec("delegate_task", "Run a focused subtask in a fresh Jeden session and return its result; requires exec-tier approval", json!({"task": {"type": "string"}, "maxSteps": {"type": "number"}}), vec!["task"]),
        tool_spec("memory", "Remember and recall durable scoped notes across Jeden sessions", json!({"op": {"type": "string"}, "text": {"type": "string"}, "query": {"type": "string"}, "tags": {"type": "array"}, "limit": {"type": "number"}, "kind": {"type": "string"}, "scope": {"type": "object"}, "confidence": {"type": "number"}}), vec!["op"]),
        tool_spec("mcp_list_tools", "List tools from a configured stdio MCP server", json!({"server": {"type": "string"}, "timeoutMs": {"type": "number"}}), vec!["server"]),
        tool_spec("mcp_call_tool", "Call one tool on a configured stdio MCP server", json!({"server": {"type": "string"}, "tool": {"type": "string"}, "args": {"type": "object"}, "timeoutMs": {"type": "number"}}), vec!["server", "tool"]),
        tool_spec("mcp_list_resources", "List resources from a configured stdio MCP server", json!({"server": {"type": "string"}, "timeoutMs": {"type": "number"}}), vec!["server"]),
        tool_spec("mcp_read_resource", "Read one resource from a configured stdio MCP server", json!({"server": {"type": "string"}, "uri": {"type": "string"}, "timeoutMs": {"type": "number"}}), vec!["server", "uri"]),
        tool_spec("mcp_list_prompts", "List prompts from a configured stdio MCP server", json!({"server": {"type": "string"}, "timeoutMs": {"type": "number"}}), vec!["server"]),
        tool_spec("mcp_get_prompt", "Get one prompt from a configured stdio MCP server", json!({"server": {"type": "string"}, "name": {"type": "string"}, "args": {"type": "object"}, "timeoutMs": {"type": "number"}}), vec!["server", "name"]),
    ];
    let mut seen = specs
        .iter()
        .filter_map(|spec| {
            spec.get("function")
                .and_then(|f| f.get("name"))
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .collect::<BTreeSet<_>>();
    for tool in crate::tools::list_tools(cwd) {
        if seen.contains(&tool.name) {
            continue;
        }
        let parameters = if tool.input.get("type").is_some() {
            tool.input.clone()
        } else {
            json!({"type": "object", "properties": tool.input})
        };
        specs.push(json!({
            "type": "function",
            "function": {
                "name": tool.name,
                "description": tool.description,
                "parameters": parameters,
            }
        }));
        seen.insert(
            specs
                .last()
                .and_then(|spec| spec.get("function"))
                .and_then(|f| f.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        );
    }
    specs.retain(|spec| {
        spec.get("function")
            .and_then(|function| function.get("name"))
            .and_then(Value::as_str)
            .is_none_or(crate::tool_runtime::tool_allowed_by_env)
    });
    specs
}

fn tool_spec(name: &str, description: &str, properties: Value, required: Vec<&str>) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": name,
            "description": description,
            "parameters": {
                "type": "object",
                "properties": properties,
                "required": required,
                "additionalProperties": false,
            }
        }
    })
}

pub(crate) fn system_prompt_checked(cwd: &Path) -> Result<String, String> {
    let config = crate::load_config(cwd);
    let ui_lang = ui_language(&config);
    let language = language_prompt_section(&ui_lang);
    let contract = engineering_contract_section(&ui_lang);
    let policy = crate::context::ContextPolicy::load(cwd, &config)?;
    let tools = crate::tools::list_tools(cwd)
        .into_iter()
        .map(|tool| format!("- {}: {}", tool.name, tool.description))
        .collect::<Vec<_>>()
        .join("\n");
    let memory = memory_guidance_for_prompt(cwd)
        .map(|summary| {
            format!(
                "\n\nMemory Guidance (heuristic; verify against current repo before acting):\n{}",
                summary
            )
        })
        .unwrap_or_default();
    let prompt = format!("You are Jeden, Wisent's private agent harness.\n\nRules:\n- Answer with {{\"action\":\"final\",\"text\":\"your concise answer\"}} when done.\n- Use tool calls when the model-router supports native tool_calls, or answer with {{\"action\":\"tool\",\"tool\":\"tool_name\",\"input\":{{...}}}}.\n- Do not create tests unless the user explicitly asks.\n- Do not create docs unless the user explicitly asks.\n- Do not invent files, command outputs, or tool results.\n- Tool approval uses read/write/exec tiers. Write-tier tools mutate files or session state; exec-tier tools run code/processes or spawn agents. The active /approval policy, --allow-* flags, and --yolo decide whether a call runs or prompts.\n\n{language}\n\n{contract}\n\nDelegation via delegate_task:\n- Scope before you spawn: read the request, map the work, name independent slices. Never outsource the top-level plan or the user's intent.\n- Spawn-one-then-wait is a bug: either fan out real independent slices in parallel or do it inline. Prerequisites every slice depends on run inline first.{}{}\n\nExecutable Rust tools:\n{}", memory, policy.system_injection(), tools);
    Ok(policy.protect_model_text(&prompt))
}

fn language_prompt_section(language: &UiLanguage) -> String {
    if language.code() == "pl" {
        // The language instruction is itself written in the target language.
        return "Język: odpowiadaj w języku oznaczonym kodem ISO 639 \"pl\" niezależnie od języka wiadomości użytkownika.\nKod, ścieżki plików, nazwy narzędzi, polecenia i identyfikatory techniczne pozostawiaj bez tłumaczenia.".to_string();
    }
    let line = if language.is_auto() {
        "Language: answer in the language of the user's current message; switch languages when the user does."
            .to_string()
    } else {
        format!(
            "Language: answer in the language identified by ISO 639 code \"{}\" regardless of the user's message language.",
            language.code()
        )
    };
    format!(
        "{line}\nKeep code, file paths, tool names, commands, and technical identifiers untranslated."
    )
}

/// Prose engineering contract; contract-critical syntax (the Rules block, the
/// action protocol, and the tool registry) is assembled elsewhere in the
/// prompt and stays English always. Any code without a variant falls back to
/// English.
fn engineering_contract_section(language: &UiLanguage) -> &'static str {
    match language.code() {
        "pl" => "Kontrakt inżynieryjny:\n- Nigdy nie poprzestawaj na pierwszej sensownej odpowiedzi, jeśli kolejne wywołanie narzędzia może zmniejszyć niepewność; puste lub podejrzanie wąskie wyszukanie oznacza ponowną próbę inną strategią, a nie zgadywanie.\n- Zbadaj temat przed edycją: czytaj całe sekcje, nie wycinki; stosuj istniejące konwencje repozytorium — druga konwencja obok istniejącej jest zabroniona. Jeśli plik zmienił się od momentu odczytu, przeczytaj go ponownie.\n- Rozwiązuj problemy u źródła; usuwaj przestarzały kod zamiast dodawać obok niego; preferuj aktualizowanie istniejących plików zamiast tworzenia nowych.\n- Twierdzenia o kodzie, narzędziach lub źródłach muszą być ugruntowane w wynikach narzędzi; wszystko, czego nie zaobserwowano bezpośrednio, oznacz jako [INFERENCE]. Wyniki narzędzi są weryfikacją — nie audytuj ponownie własnych zastosowanych edycji.\n- Nigdy nie wydawaj częściowej pracy jako ukończonej: żadnych stubów, placeholderów ani fałszywych fallbacków; nigdy po cichu nie zawężaj żądanego zakresu; nigdy nie dostarczaj poprawki na objaw zamiast prawdziwej przyczyny. Jeśli istnieją kryteria akceptacji, wszystkie muszą przechodzić.\n- Nigdy nie opowiadaj o limitach sesji, budżetach tokenów ani szacunkach nakładu pracy; po prostu wykonaj pracę.",
        _ => "Engineering contract:\n- Never stop at the first plausible answer when another tool call would cut uncertainty; an empty or suspiciously narrow lookup means retry with a different strategy, not a guess.\n- Research before editing: read sections, not snippets; reuse the repo's existing conventions — a second convention beside an existing one is prohibited. Re-read if the file changed since you read it.\n- Fix problems at the source; remove obsolete code rather than adding beside it; prefer updating existing files over creating new ones.\n- Claims about code, tools, or sources must be grounded in tool results; mark anything not directly observed as [INFERENCE]. Tool results are the verification — do not re-audit your own applied edits.\n- Never yield partial work as done: no stubs, placeholders, or fake fallbacks; never silently shrink the requested scope; never ship the symptom fix instead of the real cause. If acceptance criteria exist, all of them must pass.\n- Never narrate session limits, token budgets, or effort estimates; just do the work.",
    }
}

pub(in crate::agent) fn prepare_outbound_messages(
    cwd: &Path,
    messages: &[Value],
    attachments: &[crate::model_router::ModelAttachment],
) -> Result<Vec<Value>, String> {
    if let Some((index, _)) = messages
        .iter()
        .enumerate()
        .find(|(_, message)| !message.get("content").is_some_and(Value::is_string))
    {
        return Err(format!(
            "durable conversation message {index} has non-text content; multimodal parts must remain provider-bound"
        ));
    }
    let config = crate::load_config(cwd);
    let outbound = crate::context::prepare_model_messages(cwd, &config, messages)?;
    crate::model_router::with_attachments(outbound, attachments)
}
