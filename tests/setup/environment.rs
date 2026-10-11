use super::{report::checklist_text, setup_picker, setup_state, SlashContext};
use std::path::PathBuf;

#[test]
fn environment_read_failure_stays_unknown_until_same_process_recovery() {
    let home = PathBuf::from(std::env::var_os("HOME").expect("isolated HOME is required"))
        .canonicalize()
        .expect("resolve isolated HOME");
    let build = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(".build")
        .canonicalize()
        .expect("qualification runner creates the ignored build directory");
    assert!(
        home.starts_with(&build) && home != build,
        "HOME must be isolated inside .build"
    );
    for key in [
        "BRAMA_URL",
        "WISENT_APP_AGENT_ID",
        "WISENT_APP_AGENT_AUTH_SECRET",
    ] {
        assert!(
            std::env::var_os(key).is_none(),
            "{key} must not mask the file under qualification"
        );
    }
    let revision =
        std::env::var("JEDEN_TEST_SOURCE_REVISION").expect("exact source revision required");
    let url =
        std::env::var("JEDEN_TEST_SETUP_BRAMA_URL").expect("actual declared Brama URL required");
    let agent = std::env::var("JEDEN_TEST_SETUP_AGENT_ID")
        .expect("actual declared agent identity required");
    assert!(!url.is_empty() && !url.contains(['\r', '\n']));
    assert!(!agent.is_empty() && !agent.contains(['\r', '\n']));
    let directory = home.join(".jeden");
    std::fs::create_dir(&directory).expect("isolated HOME must not hold existing Jeden state");
    let path = directory.join(".env");
    let original = format!("BRAMA_URL={url}\nWISENT_APP_AGENT_ID={agent}\n");
    std::fs::write(&path, &original).expect("write isolated nonsecret configuration");
    let context = SlashContext {
        cwd: &home,
        model: None,
        session_root: &directory,
    };
    let readable = setup_state(&context);
    assert_eq!(readable.brama_url.as_deref(), Some(url.as_str()));
    assert_eq!(readable.agent_id.as_deref(), Some(agent.as_str()));
    assert!(readable.env_file_error.is_none());
    let retained = directory.join("retained.env");
    std::fs::rename(&path, &retained).expect("retain the test-owned environment file");
    std::fs::create_dir(&path).expect("make the file path unreadable as text");
    let cause = std::fs::read_to_string(&path)
        .expect_err("the environment path is a directory")
        .to_string();
    let unknown = setup_state(&context);
    assert!(unknown.brama_url.is_none());
    assert!(unknown.agent_id.is_none());
    let error = unknown
        .env_file_error
        .expect("the actual read failure must remain visible");
    assert!(error.contains(&cause));
    assert!(error.contains(&*path.to_string_lossy()));
    let picker = setup_picker(&context).expect("setup keeps unrelated controls usable");
    for item in &picker.items {
        if let Some(command) = &item.command {
            assert!(
                item.disabled
                    || !(command.starts_with("/setup brama-url")
                        || command.starts_with("/setup agent-id")),
                "unknown file values must not offer replacement: {command}"
            );
        }
    }
    assert!(picker.items.iter().any(|item| item.detail.contains(&cause)));
    let checklist = checklist_text(&context);
    assert!(checklist.contains(&cause));
    assert!(!checklist.contains("/setup brama-url"));
    assert!(!checklist.contains("/setup agent-id"));
    assert_eq!(
        std::fs::read_to_string(&retained).expect("retained environment"),
        original
    );
    std::fs::remove_dir(&path).expect("remove isolated refusal condition");
    std::fs::rename(&retained, &path).expect("restore isolated environment");
    let recovered = setup_state(&context);
    assert_eq!(recovered.brama_url, readable.brama_url);
    assert_eq!(recovered.agent_id, readable.agent_id);
    assert!(recovered.env_file_error.is_none());
    assert_eq!(
        std::fs::read_to_string(&path).expect("recovered environment"),
        original
    );
    std::fs::remove_file(path).expect("remove test-owned nonsecret environment");
    std::fs::remove_dir(directory).expect("remove test-owned environment directory");
    eprintln!(
        "setup source={revision} isolated={} refusal={error}",
        home.display()
    );
}
