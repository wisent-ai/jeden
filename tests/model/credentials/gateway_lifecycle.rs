//! Real model API calls across terminated, process-owned Brama children.
//! Run only this ignored case in a fresh test process whose HOME is the
//! dedicated, already-signed-in JEDEN_TEST_GATEWAY_HOME beneath .build.
//! No provider, subscription grant, response or process is simulated.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::agent::credential::private_gateway::process::{end, held};
use crate::agent::credential::private_gateway::{grants_file, holds_grants, running, stop};
use crate::model_router::{chat_completion, chat_completion_streaming};

// These are the product's environment protocol fields, not name-based policy.
const EXTERNAL_STATE: &[&str] = &[
    "BRAMA_URL",
    "BRAMA_TOKEN",
    "WISENT_APP_AGENT_AUTH_SECRET",
    "JEDEN_MODEL_ENDPOINT",
    "JEDEN_MODEL_KEY",
    "JEDEN_DATABASE_URL",
    "JEDEN_DATABASE_CA_FILE",
    "JEDEN_SESSION_ROOT",
];

fn required(name: &str) -> Result<String, String> {
    std::env::var(name)
        .map_err(|error| format!("{name} must name the real lifecycle fixture: {error}"))
        .and_then(|value| {
            if value.trim().is_empty() {
                Err(format!("{name} must not be empty"))
            } else {
                Ok(value)
            }
        })
}

fn require(condition: bool, cause: &str) -> Result<(), String> {
    if condition {
        Ok(())
    } else {
        Err(cause.to_owned())
    }
}

fn panic_cause(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(cause) = payload.downcast_ref::<String>() {
        cause.clone()
    } else if let Some(cause) = payload.downcast_ref::<&str>() {
        (*cause).to_owned()
    } else {
        format!(
            "non-text panic payload with Rust type {:?}",
            payload.type_id()
        )
    }
}

fn digest(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path)
        .map_err(|error| format!("cannot read evidence input {}: {error}", path.display()))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn git(root: &Path, arguments: &[&str], report: &mut Value) -> Result<String, String> {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(root)
        .output()
        .map_err(|error| format!("cannot run git {arguments:?}: {error}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    report["commands"].as_array_mut().unwrap().push(json!({
        "program": "git", "arguments": arguments, "cwd": root,
        "exitStatus": output.status.to_string(), "stdout": stdout, "stderr": stderr,
    }));
    require(
        output.status.success(),
        &format!("git {arguments:?} refused: {stderr}"),
    )?;
    Ok(stdout.trim().to_owned())
}

fn terminate_owned(report: &mut Value) -> Result<(), String> {
    let mut owned = held();
    let owned = owned
        .as_mut()
        .ok_or("the actual model request left no owned gateway child")?;
    let gateway = owned
        .route
        .as_ref()
        .map_err(|cause| format!("the actual model request left a failed-start child: {cause}"))?;
    let pid = owned.child.id();
    let endpoint = gateway.endpoint.clone();
    let bearer_digest = format!("{:x}", Sha256::digest(gateway.bearer.as_bytes()));
    let status = end(&mut owned.child).map_err(|error| {
        format!("cannot terminate and reap isolated gateway process {pid}: {error}")
    })?;
    report["terminatedChildren"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "pid": pid, "endpoint": endpoint, "bearerSha256": bearer_digest,
            "exitStatus": status.to_string(),
        }));
    Ok(())
}

fn exercise(root: &Path, area: &Path, report: &mut Value) -> Result<(), String> {
    let revision = required("JEDEN_TEST_SOURCE_REVISION")?;
    require(
        git(root, &["rev-parse", "HEAD"], report)? == revision,
        "the checkout does not match JEDEN_TEST_SOURCE_REVISION",
    )?;
    require(
        git(
            root,
            &["status", "--porcelain", "--untracked-files=all"],
            report,
        )?
        .is_empty(),
        "the lifecycle journey requires a clean, committed checkout",
    )?;
    let short = git(root, &["rev-parse", "--short", &revision], report)?;
    require(
        crate::JEDEN_VERSION.ends_with(&format!(".{short}")),
        "the product linked into the test does not embed the candidate revision",
    )?;
    report["sourceRevision"] = json!(revision);
    report["productVersion"] = json!(crate::JEDEN_VERSION);
    report["testExecutableSha256"] = json!(digest(
        &std::env::current_exe()
            .map_err(|error| format!("cannot identify the executing test binary: {error}"))?
    )?);
    require(
        holds_grants()?,
        "the isolated HOME holds no real signed-in subscription grant",
    )?;
    report["grantsSha256Before"] = json!(digest(&grants_file())?);
    let model = required("JEDEN_TEST_MODEL")?;
    let prompt_path = PathBuf::from(required("JEDEN_TEST_PROMPT_FILE")?);
    let expected_path = PathBuf::from(required("JEDEN_TEST_EXPECTED_RESPONSE_FILE")?);
    let expected_refusal = required("JEDEN_TEST_GATEWAY_REFUSAL")?;
    let prompt = std::fs::read_to_string(&prompt_path).map_err(|error| {
        format!(
            "cannot read real model prompt {}: {error}",
            prompt_path.display()
        )
    })?;
    let expected = std::fs::read_to_string(&expected_path).map_err(|error| {
        format!(
            "cannot read expected response {}: {error}",
            expected_path.display()
        )
    })?;
    require(
        !prompt.trim().is_empty() && !expected.trim().is_empty(),
        "the real model prompt and its expected response must be explicit and nonempty",
    )?;
    report["promptSha256"] = json!(digest(&prompt_path)?);
    report["expectedResponseSha256"] = json!(digest(&expected_path)?);
    let workspace = area.join("workspace");
    std::fs::create_dir_all(&workspace).map_err(|error| {
        format!(
            "cannot create lifecycle workspace {}: {error}",
            workspace.display()
        )
    })?;
    let args = crate::parse_args(vec![
        "run".into(),
        prompt.clone(),
        "--model-only".into(),
        "--model".into(),
        model,
        "--cwd".into(),
        workspace.to_string_lossy().into_owned(),
    ])?;
    let router = crate::agent::model_router_config(&crate::load_config(&workspace), &args);
    require(
        router.own_gateway && !router.via_brama,
        "the real CLI configuration did not select its own subscription-backed gateway",
    )?;
    if let Some(cause) = &router.config_error {
        return Err(cause.clone());
    }
    let original = running()?.ok_or("configuration selected no live owned gateway")?;
    require(
        router.url == original.endpoint && router.bearer_token == original.bearer,
        "the original cached configuration does not identify the child being exercised",
    )?;
    let messages = vec![json!({ "role": "user", "content": prompt })];
    let first = chat_completion(&router, messages.clone(), None, &[])?;
    report["baseline"] = json!({ "content": first.content });
    require(
        first.content.trim() == expected.trim(),
        "the baseline real model response differs",
    )?;

    terminate_owned(report)?;
    let buffered = chat_completion(&router, messages.clone(), None, &[])?;
    report["bufferedAfterTermination"] = json!({ "content": buffered.content });
    require(
        buffered.content.trim() == expected.trim(),
        "a buffered request using the original configuration did not survive child replacement",
    )?;
    let replacement = running()?.ok_or("the buffered request left no live replacement gateway")?;
    require(
        replacement.bearer != original.bearer,
        "the replacement did not receive a distinct, real gateway bearer",
    )?;

    terminate_owned(report)?;
    let mut visible = String::new();
    let streamed = chat_completion_streaming(
        &router,
        messages.clone(),
        None,
        &[],
        &mut |piece| {
            visible.push_str(piece);
            true
        },
        &mut |_| {},
        &|| false,
    )
    .map_err(|error| error.message)?;
    report["streamedAfterTermination"] = json!({
        "content": streamed.completion.content, "visibleContent": visible,
    });
    require(
        streamed.completion.content.trim() == expected.trim(),
        "a streamed request using the original configuration did not survive child replacement",
    )?;
    let final_gateway =
        running()?.ok_or("the streamed request left no live replacement gateway")?;
    require(
        final_gateway.bearer != replacement.bearer,
        "the streamed replacement reused the terminated child's bearer",
    )?;
    require(
        router.url == original.endpoint && router.bearer_token == original.bearer,
        "the journey rebuilt or changed the cached configuration instead of exercising dispatch",
    )?;
    report["grantsSha256After"] = json!(digest(&grants_file())?);

    // Move, never copy or edit, the isolated grant after its owner is reaped.
    // Restore it only after the refusal's replacement child has stopped.
    terminate_owned(report)?;
    let grants = grants_file();
    let retained = area.join("retained-grants.json");
    std::fs::rename(&grants, &retained).map_err(|error| {
        format!(
            "cannot isolate grants {} as {}: {error}",
            grants.display(),
            retained.display()
        )
    })?;
    let refused = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let buffered = chat_completion(&router, messages.clone(), None, &[])
            .map(|completion| completion.content);
        let mut visible = String::new();
        let streamed = chat_completion_streaming(
            &router,
            messages,
            None,
            &[],
            &mut |piece| {
                visible.push_str(piece);
                true
            },
            &mut |_| {},
            &|| false,
        )
        .map(|completion| completion.completion.content)
        .map_err(|error| error.message);
        (buffered, streamed, visible)
    }));
    report["withoutGrants"] = match &refused {
        Ok((buffered, streamed, visible)) => json!({
            "buffered": buffered, "streamed": streamed, "visibleContent": visible,
        }),
        Err(cause) => json!({ "panic": panic_cause(cause.as_ref()) }),
    };
    let stopped = stop();
    report["refusalCleanup"] = json!(stopped.as_ref().map(|_| "stopped").map_err(String::as_str));
    let restored = match &stopped {
        Ok(()) => std::fs::rename(&retained, &grants).map_err(|error| {
            format!(
                "cannot restore isolated grants {} to {}: {error}",
                retained.display(),
                grants.display()
            )
        }),
        Err(cause) => Err(format!(
            "grants remain at {} because the replacement did not stop: {cause}",
            retained.display()
        )),
    };
    report["grantRestoration"] = json!(restored
        .as_ref()
        .map(|_| "restored")
        .map_err(String::as_str));
    stopped?;
    restored?;
    let restored_digest = digest(&grants)?;
    require(
        report["grantsSha256After"].as_str() == Some(restored_digest.as_str()),
        "the refusal journey changed the retained real grant bytes",
    )?;
    report["restoredGrantsSha256"] = json!(restored_digest);
    let (buffered, streamed, visible) = refused.map_err(|cause| {
        format!(
            "the no-grants request panicked instead of returning its refusal: {}",
            panic_cause(cause.as_ref())
        )
    })?;
    require(
        visible.is_empty(),
        "the streamed no-grants request exposed model output before refusing",
    )?;
    for (kind, result) in [("buffered", buffered), ("streamed", streamed)] {
        match result {
            Err(cause) if cause.contains(&expected_refusal) => {}
            Err(cause) => return Err(format!("the real {kind} no-grants refusal lost its declared cause: {cause}")),
            Ok(_) => return Err(format!("the cached {kind} gateway configuration answered despite having no subscription grants")),
        }
    }
    Ok(())
}

#[test]
#[ignore = "requires an isolated, genuinely signed-in subscription fixture and exact test selection"]
fn cached_configuration_follows_real_owned_gateway_replacements() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let child_area = std::env::var_os("JEDEN_TEST_GATEWAY_CHILD_REPORT");
    let area = match &child_area {
        Some(path) => {
            let path =
                std::fs::canonicalize(path).expect("inspect the parent's evidence directory");
            let build = std::fs::canonicalize(root.join(".build"))
                .expect("inspect ignored build directory");
            assert!(
                path.starts_with(build),
                "child evidence must stay beneath this checkout's .build"
            );
            path
        }
        None => root
            .join(".build/model-gateway-lifecycle")
            .join(uuid::Uuid::new_v4().to_string()),
    };
    std::fs::create_dir_all(&area).expect("create the retained lifecycle evidence directory");
    if child_area.is_none() {
        let executable =
            std::env::current_exe().expect("identify the actual product test executable");
        let full_name = concat!(
            module_path!(),
            "::cached_configuration_follows_real_owned_gateway_replacements"
        );
        let name = full_name
            .strip_prefix(concat!(env!("CARGO_CRATE_NAME"), "::"))
            .expect("derive the exact libtest case name from its Rust module");
        let arguments = [name, "--ignored", "--exact", "--nocapture"];
        let mut command = Command::new(&executable);
        command
            .args(arguments)
            .current_dir(&root)
            .env(
                "HOME",
                required("JEDEN_TEST_GATEWAY_HOME").expect("name the dedicated signed-in fixture"),
            )
            .env("JEDEN_TEST_GATEWAY_CHILD_REPORT", &area);
        for name in EXTERNAL_STATE {
            command.env_remove(name);
        }
        let output = command
            .output()
            .expect("run the real product case in a fresh isolated process");
        let process = json!({
            "executable": executable, "arguments": arguments, "cwd": root,
            "exitStatus": output.status.to_string(),
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
        });
        std::fs::write(
            area.join("process.json"),
            serde_json::to_vec_pretty(&process).unwrap(),
        )
        .expect("retain the exact child command and output");
        assert!(
            output.status.success(),
            "the real isolated journey failed: {process}"
        );
        let report: Value = serde_json::from_slice(
            &std::fs::read(area.join("report.json"))
                .expect("read the child's persisted lifecycle result"),
        )
        .expect("decode actual lifecycle evidence");
        assert_eq!(
            report["verdict"], "passed",
            "the child did not persist a passing real journey"
        );
        eprintln!("gateway lifecycle evidence: {}", area.display());
        return;
    }
    let mut report = json!({ "verdict": "failed", "commands": [], "terminatedChildren": [] });
    let outcome = (|| -> Result<(), String> {
        require(
            std::env::args().any(|argument| argument == "--exact"),
            "run only this case with --exact in a fresh, isolated test process",
        )?;
        let home = std::fs::canonicalize(required("HOME")?)
            .map_err(|error| format!("cannot inspect HOME: {error}"))?;
        let fixture = std::fs::canonicalize(required("JEDEN_TEST_GATEWAY_HOME")?)
            .map_err(|error| format!("cannot inspect JEDEN_TEST_GATEWAY_HOME: {error}"))?;
        let build = std::fs::canonicalize(root.join(".build"))
            .map_err(|error| format!("cannot inspect the ignored build directory: {error}"))?;
        require(
            home == fixture && home.starts_with(&build) && home != build,
            "HOME must be the dedicated signed-in gateway fixture beneath this checkout's .build",
        )?;
        let actual_grants = std::fs::canonicalize(grants_file())
            .map_err(|error| format!("cannot inspect the isolated grant's real path: {error}"))?;
        require(
            actual_grants.starts_with(&home),
            "the dedicated fixture must not borrow a symlinked grant from outside its HOME",
        )?;
        for name in EXTERNAL_STATE {
            require(
                std::env::var_os(name).is_none(),
                &format!("{name} must be absent from this isolated test process"),
            )?;
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            exercise(&root, &area, &mut report)
        }));
        let cleanup = stop();
        if let Err(cause) = &cleanup {
            report["cleanupError"] = json!(cause);
        }
        let result = result.map_err(|cause| {
            format!(
                "the actual gateway journey panicked: {}",
                panic_cause(cause.as_ref())
            )
        })?;
        result?;
        cleanup
    })();
    if let Err(cause) = &outcome {
        report["error"] = json!(cause);
    } else {
        report["verdict"] = json!("passed");
    }
    let path = area.join("report.json");
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&report).expect("encode actual lifecycle evidence"),
    )
    .expect("retain the actual lifecycle result before asserting it");
    eprintln!("gateway lifecycle evidence: {}", path.display());
    assert!(outcome.is_ok(), "{}", outcome.unwrap_err());
}

#[cfg(unix)]
#[path = "gateway_start_failure.rs"]
mod startup;
