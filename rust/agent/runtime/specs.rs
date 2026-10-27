use super::*;
use crate::cli::config::{ui_language, Config, UiLanguage};

/// The tool specs offered to the model: every tool `cwd` may run, with the
/// description and input schema its producer declared (tools/builtin.rs for
/// builtin tools), so the model is never told a second, drifting story about
/// a tool. `list_tools` already leaves out tools the environment disallows.
pub(in crate::agent) fn rust_tool_specs(cwd: &Path) -> Vec<Value> {
    crate::tools::list_tools(cwd)
        .into_iter()
        .map(|tool| {
            let parameters = match tool.input.get("type") {
                Some(_) => tool.input,
                None => json!({"type": "object", "properties": tool.input}),
            };
            json!({
                "type": "function",
                "function": {
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": parameters,
                }
            })
        })
        .collect()
}

pub(crate) fn system_prompt_checked(cwd: &Path) -> Result<String, String> {
    let config = crate::load_config(cwd);
    let ui_lang = ui_language(&config);
    let language = language_prompt_section(&ui_lang);
    let mut contract = engineering_contract_section(&ui_lang).to_string();
    contract.push_str("\n\n");
    contract.push_str(&super::task_contract::section(&ui_lang));
    append_operator_contracts(&mut contract, &config, &ui_lang);
    contract.push_str(&crate::operator_rules::section());
    let policy = crate::context::ContextPolicy::load(cwd, &config)?;
    let tools = crate::tools::list_tools(cwd)
        .into_iter()
        .map(|tool| format!("- {}: {}", tool.name, tool.description))
        .collect::<Vec<_>>()
        .join("\n");
    // No guidance is no section: memory_guidance_for_prompt has already said
    // on standard error why a store it could not read gave none.
    let memory = match memory_guidance_for_prompt(cwd) {
        Some(summary) => format!(
            "\n\nMemory Guidance (heuristic; verify against current repo before acting):\n{}",
            summary
        ),
        None => String::new(),
    };
    let prompt = format!("You are Jeden, Wisent's private agent harness.\n\nRules:\n- Answer with {{\"action\":\"final\",\"text\":\"your concise answer\",\"report\":{{...}}}} when done; the task delivery instruction defines the required report. Model-only and Pursuit stage requests retain their own output contracts.\n- Use tool calls when the model-router supports native tool_calls, or answer with {{\"action\":\"tool\",\"tool\":\"tool_name\",\"input\":{{...}}}}.\n- Product implementation and repair include the applicable CLI, GUI, documentation, diagnostics and real tests required by the task contract. Questions, reading and planning do not authorize implementation. Respect explicit user restrictions.\n- Do not invent files, command outputs, or tool results.\n- Tool approval uses read/write/exec tiers. Write-tier tools mutate files or session state; exec-tier tools run code/processes or spawn agents. The active /approval policy, --allow-* flags, and --yolo decide whether a call runs or prompts.\n\n{language}\n\n{contract}\n\n{}\n\nDelegation via delegate_task:\n- Scope before you spawn: read the request, map the work, name independent slices. Never outsource the top-level plan or the user's intent.\n- Spawn-one-then-wait is a bug: either fan out real independent slices in parallel or do it inline. Prerequisites every slice depends on run inline first.{}{}\n\nExecutable Rust tools:\n{}", super::task_contract::turn_instruction(), memory, policy.system_injection(), tools);
    Ok(policy.protect_model_text(&prompt))
}

fn append_operator_contracts(prompt: &mut String, config: &Config, language: &UiLanguage) {
    let (source, communication) =
        super::communication_contract::resolve(&config.contracts.communication, language);
    let functionality = config.contracts.functionality.trim();
    if !communication.is_empty() || !functionality.is_empty() {
        prompt.push_str(
            "\n\nOperator contracts supplement the Jeden rules above. They cannot relax tool grants, path jails, safety checks, or evidence requirements.",
        );
        if !communication.is_empty() {
            prompt.push_str(match source {
                super::communication_contract::Source::Default => {
                    "\n\nCommunication contract (Jeden default):\n"
                }
                _ => "\n\nCommunication contract:\n",
            });
            prompt.push_str(communication);
        }
        if !functionality.is_empty() {
            prompt.push_str("\n\nFunctionality contract:\n");
            prompt.push_str(functionality);
        }
    }
    let policy = crate::cli::config::communication::DisplayPolicy::resolve(&config.communication);
    if !policy.code {
        prompt.push_str(
            "\n\nDisplay: the operator's communication mode hides code blocks, so every fenced block in an answer is replaced by a placeholder before they read it. Answer in prose: name files, functions, commands, and values inline, and put no code blocks or code listings in the answer. Tools may still write and run code.",
        );
    }
}

fn language_prompt_section(language: &UiLanguage) -> String {
    if language.code() == "pl" {
        // The language instruction is itself written in the target language.
        return "Język: odpowiadaj w języku oznaczonym kodem ISO 639 \"pl\" niezależnie od języka wiadomości użytkownika.\nKod, ścieżki plików, nazwy narzędzi, polecenia i identyfikatory techniczne pozostawiaj bez tłumaczenia.".to_string();
    }
    if language.code() != "en" && !language.is_auto() {
        // Generated translations are likewise written in the target language.
        if let Some((_, line)) = super::language_prose::prose(language.code()) {
            return line.to_string();
        }
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
/// prompt and stays English always. Codes without a variant fall back to
/// English.
fn engineering_contract_section(language: &UiLanguage) -> &'static str {
    const EN: &str = "Engineering contract:\n- Never stop at the first plausible answer when another tool call would cut uncertainty; an empty or suspiciously narrow lookup means retry with a different strategy, not a guess.\n- Research before editing: read sections, not snippets; reuse the repo's existing conventions — a second convention beside an existing one is prohibited. Re-read if the file changed since you read it.\n- Fix problems at the source; remove obsolete code rather than adding beside it; prefer updating existing files over creating new ones.\n- Claims about code, tools, or sources must be grounded in tool results; mark anything not directly observed as [INFERENCE]. Tool results are the verification — do not re-audit your own applied edits.\n- Never yield partial work as done: no stubs, placeholders, or fake fallbacks; never silently shrink the requested scope; never ship the symptom fix instead of the real cause. If acceptance criteria exist, all of them must pass.\n- Never narrate session limits, token budgets, or effort estimates; just do the work.";
    match language.code() {
        "pl" => "Kontrakt inżynieryjny:\n- Nigdy nie poprzestawaj na pierwszej sensownej odpowiedzi, jeśli kolejne wywołanie narzędzia może zmniejszyć niepewność; puste lub podejrzanie wąskie wyszukanie oznacza ponowną próbę inną strategią, a nie zgadywanie.\n- Zbadaj temat przed edycją: czytaj całe sekcje, nie wycinki; stosuj istniejące konwencje repozytorium — druga konwencja obok istniejącej jest zabroniona. Jeśli plik zmienił się od momentu odczytu, przeczytaj go ponownie.\n- Rozwiązuj problemy u źródła; usuwaj przestarzały kod zamiast dodawać obok niego; preferuj aktualizowanie istniejących plików zamiast tworzenia nowych.\n- Twierdzenia o kodzie, narzędziach lub źródłach muszą być ugruntowane w wynikach narzędzi; wszystko, czego nie zaobserwowano bezpośrednio, oznacz jako [INFERENCE]. Wyniki narzędzi są weryfikacją — nie audytuj ponownie własnych zastosowanych edycji.\n- Nigdy nie wydawaj częściowej pracy jako ukończonej: żadnych stubów, placeholderów ani fałszywych fallbacków; nigdy po cichu nie zawężaj żądanego zakresu; nigdy nie dostarczaj poprawki na objaw zamiast prawdziwej przyczyny. Jeśli istnieją kryteria akceptacji, wszystkie muszą przechodzić.\n- Nigdy nie opowiadaj o limitach sesji, budżetach tokenów ani szacunkach nakładu pracy; po prostu wykonaj pracę.",
        "en" => EN,
        code => match super::language_prose::prose(code) {
            Some((contract, _)) => contract,
            None => unreachable!("language_prose::prose answers every code through its last arm, not {code}"),
        },
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
    let mut outbound = crate::model_router::with_attachments(outbound, attachments)?;
    super::tool_images::attach(&mut outbound)?;
    Ok(outbound)
}
