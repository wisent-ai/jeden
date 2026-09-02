//! End-to-end verification of the Tama hook-registry loader, driving the same
//! entry points the agent turn and `/hooks` use:
//! `jeden::hooks::describe_hooks`, `fire_event` (the primitive behind
//! `user_prompt_submit`/`pretool_block`), and `pretool_block`.
//!
//! Run: `cargo test --test tama_hooks -- --nocapture`

use serde_json::json;
use std::env;
use std::fs;
use std::path::PathBuf;

fn temp_dir(tag: &str) -> PathBuf {
    let dir = env::temp_dir().join(format!("jeden-tama-hooks-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// Single test fn: the loader reads process-global env (`JEDEN_TAMA_REGISTRY`,
/// `HOME`), so the checks must not race each other across parallel tests.
#[test]
fn tama_registry_end_to_end() {
    let home = temp_dir("home");
    let cwd = temp_dir("cwd");
    // `pretool_block` puts this in the payload as `transcript_path`, which is what
    // a hook reads to find the session it is being asked about. The session here is
    // synthetic, so the path is the one this checkout would hold.
    let transcript = cwd.join("transcript.jsonl");
    env::set_var("HOME", &home);

    // 1. Disabled via empty env var: no Tama section, no blocking, silent.
    env::set_var("JEDEN_TAMA_REGISTRY", "");
    let describe = jeden::hooks::describe_hooks(&cwd);
    assert!(
        !describe.contains("Tama registry"),
        "disabled registry must be silent, got:\n{describe}"
    );
    assert!(describe.contains("No hooks configured."));
    assert_eq!(
        jeden::hooks::pretool_block(&cwd, "run_command", &json!({}), false, &transcript),
        None
    );

    // 2. Synthetic registry via JEDEN_TAMA_REGISTRY.
    let registry = cwd.join("registry.json");
    fs::write(
        &registry,
        r#"{"version":1,"events":{
            "user_prompt_submit":{"blocking":false,"hooks":[
                {"id":"echo","type":"command","command":"cat","timeout":5}]},
            "pre_tool_use:bash":{"blocking":true,"hooks":[
                {"id":"deny","type":"command","command":"exit 3","timeout":5}]}
        }}"#,
    )
    .expect("write synthetic registry");
    env::set_var("JEDEN_TAMA_REGISTRY", &registry);

    // `/hooks` shows the source, resolved path, and per-event counts.
    let describe = jeden::hooks::describe_hooks(&cwd);
    println!("--- /hooks with synthetic registry ---\n{describe}\n");
    assert!(describe.contains("Tama registry ("));
    assert!(describe.contains(&registry.display().to_string()));
    assert!(describe.contains("user_prompt_submit -> UserPromptSubmit [*] 1 hook(s)"));
    assert!(describe.contains(
        "pre_tool_use:bash -> PreToolUse [^(run_command|run_process)$] 1 hook(s), blocking"
    ));

    // The echo hook fires on user-prompt: drive the same firing path
    // `user_prompt_submit` uses (`fire_event` with the UserPromptSubmit
    // payload); `cat` echoes the payload back on stdout.
    let payload = json!({ "event": "UserPromptSubmit", "prompt": "hello-jeden-tama", "cwd": cwd });
    let outcomes = jeden::hooks::fire_event(&cwd, "UserPromptSubmit", "", &payload, false);
    assert!(
        outcomes
            .iter()
            .any(|o| o.exit_code == 0 && o.stdout.contains("hello-jeden-tama")),
        "echo hook should have fired and echoed the payload, got: {outcomes:?}"
    );

    // The blocking deny hook (exit 3) blocks run_command and run_process
    // pre-tool calls, but not tools outside the bash matcher.
    let blocked = jeden::hooks::pretool_block(
        &cwd,
        "run_command",
        &json!({"command": "ls"}),
        false,
        &transcript,
    );
    assert!(blocked.is_some(), "run_command should be blocked");
    assert!(
        jeden::hooks::pretool_block(&cwd, "run_process", &json!({}), false, &transcript).is_some(),
        "run_process should be blocked"
    );
    assert_eq!(
        jeden::hooks::pretool_block(&cwd, "write_file", &json!({}), false, &transcript),
        None,
        "write_file is outside the bash matcher and must not be blocked"
    );

    // 3. Real hooks-rotator registry: parse only (describe executes nothing),
    // printing the per-event mapped counts.
    let real = PathBuf::from("/Users/lukaszbartoszcze/Documents/CodingProjects/Wisent/hooks-rotator/shared-hooks/registry.json");
    if real.is_file() {
        env::set_var("JEDEN_TAMA_REGISTRY", &real);
        let describe = jeden::hooks::describe_hooks(&cwd);
        println!("--- /hooks with real registry ---\n{describe}\n");
        assert!(describe.contains("managedBy jeden-unified-hooks"));
        assert!(describe.contains("stop -> Stop [*] 16 hook(s), blocking"));
        assert!(describe.contains(
            "pre_tool_use:bash -> PreToolUse [^(run_command|run_process)$] 21 hook(s), blocking"
        ));
        assert!(describe.contains("user_prompt_submit -> UserPromptSubmit [*] 3 hook(s)"));
    } else {
        println!("real registry not present at {}, skipped", real.display());
    }

    env::remove_var("JEDEN_TAMA_REGISTRY");
    let _ = fs::remove_dir_all(&home);
    let _ = fs::remove_dir_all(&cwd);
}
