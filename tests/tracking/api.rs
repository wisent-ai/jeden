use jeden::tool_runtime::{
    execute,
    runtime_ops::{ArtifactSink, CancellationToken, OperationContext},
    ToolRuntime,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    env,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

static ENVIRONMENT: std::sync::Mutex<()> = std::sync::Mutex::new(());
mod recall;

struct SessionRoot(Option<OsString>);
impl Drop for SessionRoot {
    fn drop(&mut self) {
        match self.0.take() {
            Some(value) => env::set_var("JEDEN_SESSION_ROOT", value),
            None => env::remove_var("JEDEN_SESSION_ROOT"),
        }
    }
}
fn required(name: &str) -> Result<String, String> {
    let value = env::var(name).map_err(|cause| format!("{name} is required: {cause}"))?;
    if value.trim().is_empty() {
        return Err(format!("{name} is required and cannot be empty"));
    }
    Ok(value)
}
fn runtime<'a>(
    cwd: &'a Path,
    artifacts: &'a Path,
    session: Option<String>,
    write: bool,
    command: bool,
) -> ToolRuntime<'a> {
    let operation = OperationContext::new(
        CancellationToken::new(),
        ArtifactSink::new(artifacts),
        cwd.to_path_buf(),
    );
    let id = operation.operation_id().to_owned();
    ToolRuntime {
        cwd,
        artifact_dir: Some(artifacts),
        operation: operation.with_identity(id, session, None, None),
        allow_write: write,
        allow_command: command,
        interactive: false,
        ask_user: None,
    }
}
fn export(session: &Path, destination: &Path, report: &mut Value) -> Result<Value, String> {
    let output = Command::new(env!("CARGO_BIN_EXE_jeden"))
        .arg("export")
        .arg(session)
        .arg(destination)
        .stdin(Stdio::null())
        .output()
        .map_err(|cause| format!("export {}: {cause}", session.display()))?;
    report["commands"]
        .as_array_mut()
        .ok_or("report.commands is not an array")?
        .push(json!({
            "program": env!("CARGO_BIN_EXE_jeden"), "args": ["export", session, destination],
            "exit": output.status.code(), "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
        }));
    if !output.status.success() {
        return Err(format!(
            "real export refused {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let bytes = fs::read(destination)
        .map_err(|cause| format!("read {}: {cause}", destination.display()))?;
    serde_json::from_slice(&bytes)
        .map_err(|cause| format!("decode {}: {cause}", destination.display()))
}
fn outcome<'a>(export: &'a Value, tool: &str, path: &str) -> Result<&'a Value, String> {
    let events = export["events"]
        .as_array()
        .ok_or("session export has no events array")?;
    let mut requests = events.iter().filter(|event| {
        event["type"] == "interaction"
            && event["data"]["type"] == "operation_request"
            && event["data"]["operationKind"] == "tool"
            && event["data"]["operation"] == tool
            && event["data"]["input"]["arguments"]["path"] == path
    });
    let request = requests
        .next()
        .ok_or_else(|| format!("no execution request for {tool} {path}"))?;
    if requests.next().is_some() {
        return Err(format!("ambiguous execution identity for {tool} {path}"));
    }
    let id = request["id"]
        .as_str()
        .ok_or("execution request has no event id")?;
    let mut results = events.iter().filter(|event| {
        event["data"]["type"] == "operation_result"
            && event["data"]["requestEvent"].as_str() == Some(id)
    });
    let result = results
        .next()
        .ok_or_else(|| format!("request {id} has no terminal result"))?;
    if results.next().is_some() {
        return Err(format!("request {id} has multiple terminal results"));
    }
    Ok(&result["data"]["outcome"])
}
fn exercise(root: &Path, report: &mut Value) -> Result<(), String> {
    let workspace = root.join("workspace");
    let sessions = workspace.join(".jeden/sessions");
    let artifacts = root.join("artifacts");
    for directory in [&workspace, &sessions, &artifacts] {
        fs::create_dir_all(directory)
            .map_err(|cause| format!("create {}: {cause}", directory.display()))?;
    }
    let _restore = SessionRoot(env::var_os("JEDEN_SESSION_ROOT"));
    env::set_var("JEDEN_SESSION_ROOT", &sessions);
    let marker = uuid::Uuid::new_v4().to_string();
    let standalone = runtime(&workspace, &artifacts, None, true, false);
    let written = execute(
        &standalone,
        "write_file",
        &json!({"path":"written.txt", "content":marker}),
    );
    report["write"] = json!(&written);
    written?;
    let actual = fs::read_to_string(workspace.join("written.txt"))
        .map_err(|cause| format!("read real written file: {cause}"))?;
    if actual != marker {
        return Err("write_file did not persist the supplied content".into());
    }
    let mut entries =
        fs::read_dir(&sessions).map_err(|cause| format!("read native sessions: {cause}"))?;
    let session = entries
        .next()
        .ok_or("standalone execution created no native session")?
        .map_err(|cause| format!("read native session: {cause}"))?
        .path();
    match entries.next() {
        Some(Err(cause)) => return Err(format!("read another native session: {cause}")),
        Some(Ok(entry)) => {
            return Err(format!(
                "ambiguous native session: {}",
                entry.path().display()
            ))
        }
        None => {}
    }
    let identity = session
        .to_str()
        .ok_or("native fixture session path is not UTF-8")?
        .to_owned();
    let readonly = runtime(&workspace, &artifacts, Some(identity.clone()), false, false);
    let refused = execute(
        &readonly,
        "write_file",
        &json!({"path":"refused.txt", "content":marker}),
    );
    report["refusal"] = json!(&refused);
    if refused.is_ok() {
        return Err("write_file accepted a caller without write permission".into());
    }
    match fs::metadata(workspace.join("refused.txt")) {
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {}
        Err(cause) => return Err(format!("inspect refused file: {cause}")),
        Ok(_) => return Err("a refused write changed the filesystem".into()),
    }
    let before = export(&session, &root.join("before-relocation.json"), report)?;
    if outcome(&before, "write_file", "written.txt")?["status"] != "completed" {
        return Err("the materialized write is not recorded as completed".into());
    }
    if outcome(&before, "write_file", "refused.txt")?["status"] != "failed" {
        return Err("the denied write is not recorded as failed".into());
    }
    let relocated = workspace.join("relocated-session");
    let destination = relocated
        .to_str()
        .ok_or("relocated fixture session path is not UTF-8")?;
    let command = required("JEDEN_TEST_MOVE_PROGRAM")?;
    let movable = runtime(&workspace, &artifacts, Some(identity.clone()), true, true);
    let moved = execute(
        &movable,
        "run_process",
        &json!({"command":command, "args":[identity, destination]}),
    );
    report["relocation"] = json!(&moved);
    let cause = moved
        .err()
        .ok_or("a result with no writable original ledger was reported as recorded")?;
    match fs::metadata(&session) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("inspect original ledger location: {error}")),
        Ok(_) => return Err("the real move program did not relocate the isolated session".into()),
    }
    let observed = fs::metadata(relocated.join("transcript.jsonl"))
        .map_err(|error| format!("inspect relocated transcript: {error}"))?;
    if !observed.is_file() {
        return Err("relocation did not preserve the real transcript file".into());
    }
    if !cause.contains(destination)
        || !cause.contains(session.to_str().ok_or("original session is not UTF-8")?)
    {
        return Err("ledger-write refusal lost the successful operation's observed destination or failed ledger path".into());
    }
    let after = export(&relocated, &root.join("after-relocation.json"), report)?;
    let events = after["events"]
        .as_array()
        .ok_or("relocated export has no events array")?;
    let request = events
        .iter()
        .find(|event| {
            event["data"]["type"] == "operation_request"
                && event["data"]["operation"] == "run_process"
        })
        .ok_or("the move ran without its durable pre-execution request")?;
    let id = request["id"]
        .as_str()
        .ok_or("move request has no event identity")?;
    if events.iter().any(|event| {
        event["data"]["type"] == "operation_result"
            && event["data"]["requestEvent"].as_str() == Some(id)
    }) {
        return Err("a failed result write fabricated a persisted terminal receipt".into());
    }
    report["observedDestination"] = json!(relocated);
    report["exports"] = json!([before, after]);
    Ok(())
}

#[test]
#[ignore = "requires a real move executable, packaged sandbox helper and committed source revision"]
fn standalone_calls_keep_actual_effects_and_refusals() {
    let _environment = ENVIRONMENT
        .lock()
        .expect("serialize fixture session-root ownership");
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(".build/tracking")
        .join(uuid::Uuid::new_v4().to_string());
    fs::create_dir_all(&root).expect("create retained real tracking journey directory");
    let source = required("JEDEN_TEST_SOURCE_REVISION");
    let mut report = json!({
        "commands": [], "verdict":"failed",
        "journeySha256": format!("{:x}", Sha256::digest(include_bytes!("api.rs"))),
    });
    let result = source.and_then(|revision| {
        report["sourceRevision"] = json!(revision);
        let current =
            env::current_exe().map_err(|cause| format!("locate compiled SDK journey: {cause}"))?;
        report["sdkBinary"] = json!(current);
        report["sdkSha256"] = json!(format!(
            "{:x}",
            Sha256::digest(
                fs::read(&current)
                    .map_err(|cause| format!("read SDK journey artifact: {cause}"))?
            )
        ));
        report["cliSha256"] = json!(format!(
            "{:x}",
            Sha256::digest(
                fs::read(env!("CARGO_BIN_EXE_jeden"))
                    .map_err(|cause| format!("read candidate CLI artifact: {cause}"))?
            )
        ));
        exercise(&root, &mut report)
    });
    match &result {
        Ok(()) => report["verdict"] = json!("passed"),
        Err(cause) => report["failure"] = json!(cause),
    }
    fs::write(
        root.join("report.json"),
        serde_json::to_vec_pretty(&report).expect("serialize real tracking observations"),
    )
    .expect("retain real tracking journey report");
    if let Err(cause) = result {
        panic!("{cause}; evidence: {}", root.join("report.json").display());
    }
}
