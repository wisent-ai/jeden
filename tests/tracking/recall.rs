//! Read an actual multipart session through the public tool and CLI interfaces.
use super::*;

fn exercise(root: &Path, report: &mut Value) -> Result<(), String> {
    let session = PathBuf::from(required("JEDEN_TEST_MULTIPART_SESSION")?);
    if !session.is_absolute() {
        return Err(
            "JEDEN_TEST_MULTIPART_SESSION must name an absolute real native session path".into(),
        );
    }
    let expected = required("JEDEN_TEST_MULTIPART_TEXT")?;
    let transcript = session.join("transcript.jsonl");
    let original = fs::read(&transcript).map_err(|cause| {
        format!(
            "read genuine multipart session {}: {cause}",
            transcript.display()
        )
    })?;
    report["fixtureSha256"] = json!(format!("{:x}", Sha256::digest(&original)));
    let exported = export(&session, &root.join("multipart-source.json"), report)?;
    let events = exported["events"]
        .as_array()
        .ok_or("source export has no events array")?;
    let role = events
        .iter()
        .filter_map(|event| event["data"]["messages"].as_array())
        .flatten()
        .find_map(|message| {
            let parts = message["content"].as_array()?;
            if parts.first()?.get("text")?.as_str()? != expected
                || !parts.iter().any(|part| {
                    part["type"].as_str().is_some_and(|kind| kind != "text")
                        && part.get("text").is_none()
                })
            {
                return None;
            }
            message["role"].as_str()
        })
        .ok_or("the real session has no mixed-content message beginning with the independently expected text; do not replace it with a handwritten transcript")?;
    let expected_transcript = format!("[{}]\n{expected}", role.to_ascii_uppercase());
    let workspace = root.join("workspace");
    let artifacts = root.join("artifacts");
    let sessions = workspace.join(".jeden/sessions");
    for path in [&workspace, &artifacts, &sessions] {
        fs::create_dir_all(path).map_err(|cause| format!("create {}: {cause}", path.display()))?;
    }
    let _restore = SessionRoot(env::var_os("JEDEN_SESSION_ROOT"));
    env::set_var("JEDEN_SESSION_ROOT", sessions);
    let runtime = runtime(&workspace, &artifacts, None, false, false);
    let result = execute(&runtime, "recall_conversation", &json!({"session":session}));
    report["recall"] = json!(&result);
    let value = result?;
    let recalled = value["transcript"]
        .as_str()
        .ok_or("the actual recall tool returned no transcript text")?;
    if !recalled.contains(&expected_transcript) {
        return Err(
            "text-only recall discarded a real recorded multipart text part or its speaker".into(),
        );
    }
    let after = fs::read(&transcript)
        .map_err(|cause| format!("inspect original multipart journal: {cause}"))?;
    if after != original {
        return Err("recall changed its genuine source journal".into());
    }
    report["observedText"] = json!(expected);
    report["observedRole"] = json!(role);
    report["sourceUnchanged"] = json!(true);
    Ok(())
}

#[test]
#[ignore = "requires a genuine native session with independently identified multipart text"]
fn multipart_recall_keeps_real_recorded_text() {
    let _environment = ENVIRONMENT
        .lock()
        .expect("serialize fixture session-root ownership");
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(".build/tracking")
        .join(uuid::Uuid::new_v4().to_string());
    fs::create_dir_all(&root).expect("create retained real recall journey directory");
    let mut report = json!({ "commands": [], "verdict": "failed",
        "journeySha256": format!("{:x}", Sha256::digest(include_bytes!("recall.rs"))) });
    let result = required("JEDEN_TEST_SOURCE_REVISION").and_then(|revision| {
        report["sourceRevision"] = json!(revision);
        report["cliSha256"] = json!(format!(
            "{:x}",
            Sha256::digest(
                fs::read(env!("CARGO_BIN_EXE_jeden"))
                    .map_err(|cause| format!("read actual candidate CLI: {cause}"))?
            )
        ));
        report["sdkSha256"] = json!(format!(
            "{:x}",
            Sha256::digest(
                fs::read(
                    env::current_exe()
                        .map_err(|cause| format!("locate actual SDK journey: {cause}"))?
                )
                .map_err(|cause| format!("read actual SDK journey: {cause}"))?
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
        serde_json::to_vec_pretty(&report).expect("serialize actual recall observations"),
    )
    .expect("retain actual recall observations");
    if let Err(cause) = result {
        panic!("{cause}; evidence: {}", root.join("report.json").display());
    }
}
