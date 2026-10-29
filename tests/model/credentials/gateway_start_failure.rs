//! A real Brama startup refusal followed by the host's actual ECHILD cleanup
//! refusal. The isolated child changes only its own SIGCHLD disposition;
//! neither a Child, provider, response nor system-call result is substituted.
use super::{digest, git, panic_cause, require, required, EXTERNAL_STATE};
use crate::agent::credential::private_gateway::process::held;
use crate::agent::credential::private_gateway::{described, grants_file, running, start, stop};
use serde_json::{json, Value};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

fn retain(path: &Path, value: &Value) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| format!("encode {}: {error}", path.display()))?;
    std::fs::write(path, bytes).map_err(|error| format!("write {}: {error}", path.display()))
}

fn command(
    program: &Path,
    args: &[String],
    report: &mut Value,
) -> Result<std::process::Output, String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|error| format!("execute {} {args:?}: {error}", program.display()))?;
    report["commands"].as_array_mut().ok_or("command observations are not an array")?.push(json!({
        "program": program, "arguments": args, "exitStatus": output.status.to_string(),
        "stdout": String::from_utf8_lossy(&output.stdout), "stderr": String::from_utf8_lossy(&output.stderr),
    }));
    Ok(output)
}

fn lock_observation(path: &Path) -> Result<(std::fs::File, Option<i32>), String> {
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(|error| format!("open actual renewal lease {}: {error}", path.display()))?;
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    // https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/flock.2.html
    let error = if result == -1 {
        Some(std::io::Error::last_os_error())
    } else {
        None
    };
    Ok((file, error.and_then(|error| error.raw_os_error())))
}

fn automatic_reaping() -> Result<libc::sigaction, String> {
    // SAFETY: initialized POSIX structures; only this isolated process changes SIGCHLD.
    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    let mut previous: libc::sigaction = unsafe { std::mem::zeroed() };
    action.sa_sigaction = libc::SIG_DFL;
    action.sa_flags = libc::SA_NOCLDWAIT;
    // https://pubs.opengroup.org/onlinepubs/9799919799/functions/sigemptyset.html
    if unsafe { libc::sigemptyset(&mut action.sa_mask) } == -1 {
        return Err(format!(
            "initialize SIGCHLD mask: {}",
            std::io::Error::last_os_error()
        ));
    }
    // https://pubs.opengroup.org/onlinepubs/9799919799/functions/sigaction.html
    if unsafe { libc::sigaction(libc::SIGCHLD, &action, &mut previous) } == -1 {
        return Err(format!(
            "install actual SA_NOCLDWAIT: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(previous)
}

fn restore(previous: &libc::sigaction) -> Result<(), String> {
    // https://pubs.opengroup.org/onlinepubs/9799919799/functions/sigaction.html
    if unsafe { libc::sigaction(libc::SIGCHLD, previous, std::ptr::null_mut()) } == -1 {
        return Err(format!(
            "restore SIGCHLD: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

fn exercise(root: &Path, area: &Path, report: &mut Value) -> Result<(), String> {
    require(
        std::env::args().any(|argument| argument == "--exact"),
        "run the child case alone with --exact",
    )?;
    let revision = required("JEDEN_TEST_SOURCE_REVISION")?;
    require(
        git(root, &["rev-parse", "HEAD"], report)? == revision,
        "the source revision differs",
    )?;
    require(
        git(
            root,
            &["status", "--porcelain", "--untracked-files=all"],
            report,
        )?
        .is_empty(),
        "the journey requires clean committed source",
    )?;
    let short = git(root, &["rev-parse", "--short", &revision], report)?;
    require(
        crate::JEDEN_VERSION.ends_with(&format!(".{short}")),
        "the linked product revision differs",
    )?;
    report["sourceRevision"] = json!(revision);
    report["testExecutableSha256"] = json!(digest(
        &std::env::current_exe().map_err(|error| error.to_string())?
    )?);
    let binary =
        std::fs::canonicalize(required("JEDEN_TEST_BINARY")?).map_err(|error| error.to_string())?;
    report["jedenBinarySha256"] = json!(digest(&binary)?);
    let version = command(&binary, &["--version".into()], report)?;
    require(
        version.status.success()
            && String::from_utf8_lossy(&version.stdout).contains(crate::JEDEN_VERSION),
        "the actual CLI revision differs",
    )?;
    let located = command(Path::new("which"), &["brama".into()], report)?;
    require(
        located.status.success(),
        "the real Brama executable cannot be located",
    )?;
    let brama = std::fs::canonicalize(
        String::from_utf8(located.stdout)
            .map_err(|error| error.to_string())?
            .trim(),
    )
    .map_err(|error| error.to_string())?;
    report["bramaExecutable"] = json!(brama);
    report["bramaBinarySha256"] = json!(digest(&brama)?);
    require(
        command(&brama, &["version".into()], report)?
            .status
            .success(),
        "the actual Brama version command refused",
    )?;
    let home = std::fs::canonicalize(required("HOME")?).map_err(|error| error.to_string())?;
    require(
        home == std::fs::canonicalize(area.join("home")).map_err(|error| error.to_string())?,
        "HOME must be this journey's new isolated home",
    )?;
    for name in EXTERNAL_STATE
        .iter()
        .chain(["PROBIERZ_INTAKE_URL", "PROBIERZ_INTAKE_TOKEN"].iter())
    {
        require(
            std::env::var_os(name).is_none(),
            &format!("{name} must be absent"),
        )?;
    }
    let grants = grants_file();
    let directory = grants
        .parent()
        .ok_or("the isolated grants path has no parent")?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(u32::from(libc::S_IRWXU))
        .create(directory)
        .map_err(|error| error.to_string())?;
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(u32::from(libc::S_IRUSR | libc::S_IWUSR))
        .open(&grants)
        .map_err(|error| error.to_string())?;
    file.write_all(b"[]")
        .and_then(|_| file.sync_all())
        .map_err(|error| error.to_string())?;
    drop(file);
    report["grantsSha256Before"] = json!(digest(&grants)?);
    let previous = automatic_reaping()?;
    let startup = std::panic::catch_unwind(std::panic::AssertUnwindSafe(start));
    let restored = restore(&previous);
    report["signalRestoration"] = json!(restored
        .as_ref()
        .map(|_| "restored")
        .map_err(String::as_str));
    report["startup"] = match &startup {
        Ok(Err(cause)) => json!({ "refusal": cause }),
        Ok(Ok(gateway)) => json!({ "unexpectedEndpoint": gateway.endpoint }),
        Err(cause) => json!({ "panic": panic_cause(cause.as_ref()) }),
    };
    restored?;
    let cause = match startup.map_err(|cause| panic_cause(cause.as_ref()))? {
        Err(cause) => cause,
        Ok(_) => return Err("the real invalid-grants startup unexpectedly served a route".into()),
    };
    let pid = {
        let mut state = held();
        let owned = state
            .as_mut()
            .ok_or("failed cleanup dropped the actual Child and renewal lease")?;
        let retained = owned
            .route
            .as_ref()
            .err()
            .ok_or("failed startup invented a serving route")?;
        require(
            retained == &cause,
            "the retained startup cause differs from the initiating refusal",
        )?;
        let observation = owned
            .child
            .try_wait()
            .err()
            .ok_or("the host did not produce an actual inspection refusal")?;
        report["inspection"] =
            json!({ "error": observation.to_string(), "errno": observation.raw_os_error() });
        require(
            observation.raw_os_error() == Some(libc::ECHILD),
            "the host did not expose actual ECHILD",
        )?;
        owned.child.id()
    };
    report["gatewayPid"] = json!(pid);
    let lock = directory.join("gateway.lock");
    let (probe, errno) = lock_observation(&lock)?;
    report["heldLeaseErrno"] = json!(errno);
    require(
        errno == Some(libc::EWOULDBLOCK),
        "cleanup refusal did not retain the actual OS renewal lease",
    )?;
    drop(probe);
    let inspection = running()
        .err()
        .ok_or("inspection returned an unverified route or absence")?;
    let repeated = start()
        .err()
        .ok_or("a repeated invocation served another gateway before cleanup")?;
    let status = described();
    report["repeatedRefusal"] = json!(repeated);
    report["inspectionRefusal"] = json!(inspection);
    report["status"] = json!(status);
    require(
        inspection.contains(&cause) && repeated.contains(&cause) && status.contains(&cause),
        "a normal consumer or status lost the initiating causes",
    )?;
    let args: Vec<String> = serde_json::from_str(&required("JEDEN_TEST_GATEWAY_LEASE_COMMAND")?)
        .map_err(|error| error.to_string())?;
    require(
        args.starts_with(&["token".into(), "sign-in".into()]),
        "the CLI case must invoke token sign-in",
    )?;
    let refused = command(&binary, &args, report)?;
    require(
        !refused.status.success()
            && String::from_utf8_lossy(&refused.stderr).contains(lock.to_string_lossy().as_ref()),
        "the real competing CLI did not refuse on the held renewal lease",
    )?;
    report["grantsSha256After"] = json!(digest(&grants)?);
    require(
        report["grantsSha256Before"] == report["grantsSha256After"],
        "refused startup changed the isolated input",
    )?;
    Ok(())
}

fn final_state(home: &Path, report: &mut Value) -> Result<(), String> {
    let pid = libc::pid_t::try_from(
        report["observations"]["gatewayPid"]
            .as_u64()
            .ok_or("actual gateway pid is missing")?,
    )
    .map_err(|error| error.to_string())?;
    // https://pubs.opengroup.org/onlinepubs/9799919799/functions/kill.html: signal zero only checks the actual process.
    let result = unsafe { libc::kill(pid, 0) };
    // https://pubs.opengroup.org/onlinepubs/9799919799/functions/kill.html
    let error = if result == -1 {
        Some(std::io::Error::last_os_error())
    } else {
        None
    };
    let errno = error.as_ref().and_then(std::io::Error::raw_os_error);
    report["finalProcessObservation"] =
        json!({ "result": result, "error": error.map(|error| error.to_string()), "errno": errno });
    require(
        errno == Some(libc::ESRCH),
        "the actual failed-start process is not confirmed absent",
    )?;
    let (lease, errno) = lock_observation(&home.join(".jeden/brama/gateway.lock"))?;
    report["finalLeaseErrno"] = json!(errno);
    require(
        errno.is_none(),
        "the actual renewal lease remains held after the isolated process ended",
    )?;
    drop(lease);
    let final_digest = digest(&home.join(".jeden/brama/grants.json"))?;
    report["finalGrantsSha256"] = json!(final_digest);
    require(
        report["observations"]["grantsSha256Before"].as_str() == Some(final_digest.as_str()),
        "the final grants bytes differ",
    )
}

#[test]
#[ignore = "requires the real candidate CLI and Brama on a dedicated host supporting SA_NOCLDWAIT"]
fn real_failed_start_retains_child_and_renewal_lease() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let parent = std::env::var_os("JEDEN_TEST_START_PARENT_PID");
    let area = PathBuf::from(
        required("JEDEN_TEST_START_REPORT_DIR")
            .expect("name a fresh checkout-local evidence directory"),
    );
    std::fs::create_dir_all(&area).expect("create retained source-bound evidence directory");
    let area = std::fs::canonicalize(area).expect("inspect evidence directory");
    let build =
        std::fs::canonicalize(root.join(".build")).expect("inspect checkout build directory");
    assert!(
        area.starts_with(&build) && area != build,
        "evidence must stay beneath the checkout's ignored build directory"
    );
    if parent.is_none() {
        let home = area.join("home");
        std::fs::create_dir(&home).expect("create a fresh isolated home");
        let executable = std::env::current_exe().expect("identify real product test executable");
        let full = concat!(
            module_path!(),
            "::real_failed_start_retains_child_and_renewal_lease"
        );
        let name = full
            .strip_prefix(concat!(env!("CARGO_CRATE_NAME"), "::"))
            .expect("derive exact libtest name");
        let arguments = [name, "--ignored", "--exact", "--nocapture"];
        let mut process = Command::new(&executable);
        process
            .args(arguments)
            .current_dir(&root)
            .env("HOME", &home)
            .env(
                "JEDEN_TEST_START_PARENT_PID",
                std::process::id().to_string(),
            )
            .env("JEDEN_TEST_START_REPORT_DIR", &area);
        for name in EXTERNAL_STATE
            .iter()
            .chain(["PROBIERZ_INTAKE_URL", "PROBIERZ_INTAKE_TOKEN"].iter())
        {
            process.env_remove(name);
        }
        let result = process
            .output()
            .expect("execute actual isolated product case");
        let mut report = json!({ "verdict": "failed", "child": {
            "executable": executable, "arguments": arguments, "exitStatus": result.status.to_string(),
            "stdout": String::from_utf8_lossy(&result.stdout), "stderr": String::from_utf8_lossy(&result.stderr),
        }});
        retain(&area.join("process.json"), &report)
            .expect("retain actual process result before further inspection");
        let observations = std::fs::read(area.join("child.json"))
            .map_err(|error| error.to_string())
            .and_then(|bytes| {
                serde_json::from_slice::<Value>(&bytes).map_err(|error| error.to_string())
            });
        let primary = match observations {
            Ok(observations) => {
                report["observations"] = observations;
                require(
                    result.status.success()
                        && report["observations"]["verdict"] == "held-refusal-observed",
                    "the actual child did not qualify; inspect its retained causes",
                )
            }
            Err(cause) => Err(format!("read actual child observations: {cause}")),
        };
        let final_result = final_state(&home, &mut report);
        if let Err(cause) = &primary {
            report["primaryError"] = json!(cause);
        }
        if let Err(cause) = &final_result {
            report["finalStateError"] = json!(cause);
        }
        let outcome = match (primary, final_result) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(primary), Err(final_state)) => Err(format!(
                "{primary}; final state also refused: {final_state}"
            )),
            (Err(cause), _) | (_, Err(cause)) => Err(cause),
        };
        if let Err(cause) = &outcome {
            report["error"] = json!(cause);
        } else {
            report["verdict"] = json!("passed");
        }
        retain(&area.join("report.json"), &report).expect("retain actual final-state observations");
        assert!(
            outcome.is_ok(),
            "{}; evidence: {}",
            outcome.unwrap_err(),
            area.display()
        );
        return;
    }
    let parent = parent
        .expect("the child branch has an explicit parent PID")
        .into_string()
        .expect("JEDEN_TEST_START_PARENT_PID must be Unicode")
        .parse::<libc::pid_t>()
        .expect("JEDEN_TEST_START_PARENT_PID must name the actual native parent");
    // SAFETY: getppid only observes this isolated process's actual parent.
    assert_eq!(
        unsafe { libc::getppid() },
        parent,
        "the child must be invoked by its recorded parent"
    );
    let mut report = json!({ "verdict": "failed", "commands": [], "parentPid": parent });
    let mut outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        exercise(&root, &area, &mut report)
    }))
    .map_err(|cause| panic_cause(cause.as_ref()))
    .and_then(|result| result);
    if let Some(owned) = held().as_ref() {
        report["gatewayPid"] = json!(owned.child.id());
    }
    let cleanup = stop();
    report["cleanup"] = json!(cleanup.as_ref().map(|_| "stopped").map_err(String::as_str));
    if outcome.is_ok() {
        let retained = held();
        let initiating = report["startup"]["refusal"]
            .as_str()
            .expect("the actual startup refusal was retained");
        if !cleanup
            .as_ref()
            .err()
            .is_some_and(|cause| cause.contains(initiating))
            || !retained.as_ref().is_some_and(|owned| owned.route.is_err())
        {
            outcome =
                Err("unverified cleanup dropped ownership or lost the initiating refusal".into());
        }
    }
    if let Err(cause) = &outcome {
        report["error"] = json!(cause);
    } else {
        report["verdict"] = json!("held-refusal-observed");
    }
    retain(&area.join("child.json"), &report).expect("retain actual child and lease refusals");
    assert!(outcome.is_ok(), "{}", outcome.unwrap_err());
}
