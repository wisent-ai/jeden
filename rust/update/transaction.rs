use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct InstallPaths {
    pub target: PathBuf,
    pub stage: PathBuf,
    pub backup: PathBuf,
    pub journal: PathBuf,
    pub lock: PathBuf,
    pub state: PathBuf,
}

impl InstallPaths {
    pub fn new(target: PathBuf) -> Result<Self, String> {
        let parent = target
            .parent()
            .ok_or("update target has no parent directory")?
            .to_path_buf();
        let name = target
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("update target name is not UTF-8")?
            .to_owned();
        Ok(Self {
            target,
            stage: parent.join(format!(".{name}.jeden-update.stage")),
            backup: parent.join(format!(".{name}.jeden-update.backup")),
            journal: parent.join(format!(".{name}.jeden-update.journal.json")),
            lock: parent.join(format!(".{name}.jeden-update.lock")),
            state: parent.join(format!(".{name}.jeden-update-state.json")),
        })
    }

    fn parent(&self) -> &Path {
        self.target.parent().expect("validated update target")
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum Phase {
    Prepared,
    Staged,
    BackingUp,
    BackedUp,
    Activated,
    Committed,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Journal {
    schema_version: u32,
    phase: Phase,
    from_version: String,
    to_version: String,
    artifact_sha256: String,
    previous_state: Option<InstalledState>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstalledState {
    pub schema_version: u32,
    pub version: String,
    pub artifact_sha256: String,
}

struct UpdateLock {
    path: PathBuf,
    parent: PathBuf,
    _file: File,
}

impl UpdateLock {
    fn acquire(paths: &InstallPaths) -> Result<Self, String> {
        let open = || {
            OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&paths.lock)
        };
        let mut file = match open() {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let owner = fs::read_to_string(&paths.lock)
                    .ok()
                    .and_then(|value| value.trim().parse::<u32>().ok());
                if owner.is_none() || owner.is_some_and(process_alive) {
                    return Err("another updater holds the durable update lock".into());
                }
                fs::remove_file(&paths.lock)
                    .map_err(|remove| format!("remove stale update lock: {remove}"))?;
                open().map_err(|retry| {
                    if retry.kind() == std::io::ErrorKind::AlreadyExists {
                        "another updater acquired the durable update lock".into()
                    } else {
                        format!("acquire update lock: {retry}")
                    }
                })?
            }
            Err(error) => return Err(format!("acquire update lock: {error}")),
        };
        writeln!(file, "{}", std::process::id())
            .and_then(|_| file.sync_all())
            .map_err(|error| format!("persist update lock owner: {error}"))?;
        sync_dir(paths.parent())?;
        Ok(Self {
            path: paths.lock.clone(),
            parent: paths.parent().to_path_buf(),
            _file: file,
        })
    }
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    unsafe extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }
    unsafe { kill(pid as i32, 0) == 0 }
}

#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut std::ffi::c_void;
        fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
    }
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        false
    } else {
        unsafe {
            CloseHandle(handle);
        }
        true
    }
}

#[cfg(not(any(unix, windows)))]
fn process_alive(_pid: u32) -> bool {
    true
}

impl Drop for UpdateLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        let _ = sync_dir(&self.parent);
    }
}

fn sync_dir(path: &Path) -> Result<(), String> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("fsync directory {}: {error}", path.display()))
}

fn durable_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let temp = path.with_extension("tmp");
    let bytes = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temp)
        .map_err(|error| format!("write {}: {error}", temp.display()))?;
    file.write_all(&bytes)
        .and_then(|_| file.write_all(b"\n"))
        .and_then(|_| file.sync_all())
        .map_err(|error| format!("fsync {}: {error}", temp.display()))?;
    fs::rename(&temp, path).map_err(|error| format!("replace {}: {error}", path.display()))?;
    sync_dir(path.parent().ok_or("durable JSON has no parent")?)
}

fn journal(
    paths: &InstallPaths,
    phase: Phase,
    from_version: &str,
    to_version: &str,
    artifact_sha256: &str,
    previous_state: &Option<InstalledState>,
) -> Result<(), String> {
    durable_json(
        &paths.journal,
        &Journal {
            schema_version: 1,
            phase,
            from_version: from_version.into(),
            to_version: to_version.into(),
            artifact_sha256: artifact_sha256.into(),
            previous_state: previous_state.clone(),
        },
    )
}

pub fn recover_exclusive(paths: &InstallPaths) -> Result<Option<String>, String> {
    fs::create_dir_all(paths.parent()).map_err(|error| error.to_string())?;
    let _lock = UpdateLock::acquire(paths)?;
    recover(paths)
}

fn hit(configured: Option<&str>, point: &str) -> Result<(), String> {
    if configured == Some(point) {
        Err(format!("injected updater crash at {point}"))
    } else {
        Ok(())
    }
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path)
        .map_err(|error| error.to_string())?
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).map_err(|error| error.to_string())
}
#[cfg(windows)]
fn make_executable(path: &Path) -> Result<(), String> {
    if path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("exe"))
    {
        Ok(())
    } else {
        Err("Windows updates require an .exe target".into())
    }
}
#[cfg(not(any(unix, windows)))]
fn make_executable(_path: &Path) -> Result<(), String> {
    Err("self-update executable activation is unsupported on this platform".into())
}

pub fn read_installed_state(paths: &InstallPaths) -> Result<Option<InstalledState>, String> {
    match fs::read(&paths.state) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| format!("invalid installed update state: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("read installed update state: {error}")),
    }
}

pub fn recover(paths: &InstallPaths) -> Result<Option<String>, String> {
    let bytes = match fs::read(&paths.journal) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("read update journal: {error}")),
    };
    let entry: Journal = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid update journal: {error}"))?;
    if entry.schema_version != 1 {
        return Err(format!(
            "unsupported update journal schema {}",
            entry.schema_version
        ));
    }
    match entry.phase {
        Phase::Prepared | Phase::Staged => {
            if paths.stage.exists() {
                fs::remove_file(&paths.stage)
                    .map_err(|error| format!("remove interrupted stage: {error}"))?;
            }
        }
        Phase::BackingUp | Phase::BackedUp => {
            if paths.backup.exists() {
                if paths.target.exists() {
                    fs::remove_file(&paths.target)
                        .map_err(|error| format!("remove uncertain target: {error}"))?;
                }
                fs::rename(&paths.backup, &paths.target)
                    .map_err(|error| format!("restore last-known-good: {error}"))?;
            }
            if paths.stage.exists() {
                fs::remove_file(&paths.stage)
                    .map_err(|error| format!("remove interrupted stage: {error}"))?;
            }
        }
        Phase::Activated => {
            rollback(paths)?;
            restore_state(paths, &entry.previous_state)?;
        }
        Phase::Committed => {
            if paths.backup.exists() {
                fs::remove_file(&paths.backup)
                    .map_err(|error| format!("remove committed backup: {error}"))?;
            }
            if paths.stage.exists() {
                fs::remove_file(&paths.stage)
                    .map_err(|error| format!("remove committed stage: {error}"))?;
            }
        }
    }
    sync_dir(paths.parent())?;
    fs::remove_file(&paths.journal)
        .map_err(|error| format!("remove recovered journal: {error}"))?;
    sync_dir(paths.parent())?;
    Ok(Some(format!(
        "recovered interrupted update {} -> {}",
        entry.from_version, entry.to_version
    )))
}

fn restore_state(paths: &InstallPaths, previous: &Option<InstalledState>) -> Result<(), String> {
    match previous {
        Some(state) => durable_json(&paths.state, state),
        None if paths.state.exists() => {
            fs::remove_file(&paths.state)
                .map_err(|error| format!("remove rolled-back version state: {error}"))?;
            sync_dir(paths.parent())
        }
        None => Ok(()),
    }
}

fn rollback(paths: &InstallPaths) -> Result<(), String> {
    if !paths.backup.exists() {
        return Err("cannot roll back update: last-known-good binary is missing".into());
    }
    let failed = paths.target.with_extension("jeden-update.failed");
    if failed.exists() {
        fs::remove_file(&failed).map_err(|error| error.to_string())?;
    }
    if paths.target.exists() {
        fs::rename(&paths.target, &failed)
            .map_err(|error| format!("quarantine failed update: {error}"))?;
    }
    fs::rename(&paths.backup, &paths.target)
        .map_err(|error| format!("restore last-known-good: {error}"))?;
    sync_dir(paths.parent())?;
    if failed.exists() {
        fs::remove_file(failed).map_err(|error| format!("remove failed update: {error}"))?;
    }
    Ok(())
}

pub fn install<F>(
    paths: &InstallPaths,
    artifact: &[u8],
    from_version: &str,
    to_version: &str,
    artifact_sha256: &str,
    failpoint: Option<&str>,
    mut health: F,
) -> Result<(), String>
where
    F: FnMut(&Path, &Path) -> Result<(), String>,
{
    fs::create_dir_all(paths.parent()).map_err(|error| error.to_string())?;
    let _lock = UpdateLock::acquire(paths)?;
    recover(paths)?;
    let reject_staged = |error: String| -> Result<(), String> {
        if paths.stage.exists() {
            fs::remove_file(&paths.stage)
                .map_err(|cleanup| format!("{error}; remove rejected stage: {cleanup}"))?;
        }
        if paths.journal.exists() {
            fs::remove_file(&paths.journal)
                .map_err(|cleanup| format!("{error}; remove rejected journal: {cleanup}"))?;
        }
        sync_dir(paths.parent())?;
        Err(error)
    };
    if paths.stage.exists() || paths.backup.exists() {
        return Err("orphan updater files remain after recovery".into());
    }
    let previous_state = read_installed_state(paths)?;
    journal(
        paths,
        Phase::Prepared,
        from_version,
        to_version,
        artifact_sha256,
        &previous_state,
    )?;
    hit(failpoint, "after-prepared-journal-fsync")?;
    let mut stage = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&paths.stage)
        .map_err(|error| format!("stage update: {error}"))?;
    stage
        .write_all(artifact)
        .and_then(|_| stage.sync_all())
        .map_err(|error| format!("fsync staged update: {error}"))?;
    drop(stage);
    make_executable(&paths.stage)?;
    journal(
        paths,
        Phase::Staged,
        from_version,
        to_version,
        artifact_sha256,
        &previous_state,
    )?;
    hit(failpoint, "after-stage-fsync")?;
    if paths.target.exists() {
        if let Err(error) = health(&paths.target, paths.parent()) {
            return reject_staged(format!("pre-update health failed: {error}"));
        }
    }
    if let Err(error) = health(&paths.stage, paths.parent()) {
        return reject_staged(format!("staged update health failed: {error}"));
    }
    journal(
        paths,
        Phase::BackingUp,
        from_version,
        to_version,
        artifact_sha256,
        &previous_state,
    )?;
    if paths.target.exists() {
        fs::rename(&paths.target, &paths.backup)
            .map_err(|error| format!("backup current binary: {error}"))?;
        sync_dir(paths.parent())?;
    }
    hit(failpoint, "after-backup-rename-fsync")?;
    journal(
        paths,
        Phase::BackedUp,
        from_version,
        to_version,
        artifact_sha256,
        &previous_state,
    )?;
    fs::rename(&paths.stage, &paths.target)
        .map_err(|error| format!("activate staged update: {error}"))?;
    sync_dir(paths.parent())?;
    hit(failpoint, "after-activate-rename-fsync")?;
    journal(
        paths,
        Phase::Activated,
        from_version,
        to_version,
        artifact_sha256,
        &previous_state,
    )?;
    if let Err(error) = health(&paths.target, paths.parent()) {
        rollback(paths)?;
        fs::remove_file(&paths.journal).map_err(|remove| {
            format!("{error}; rollback succeeded but journal cleanup failed: {remove}")
        })?;
        sync_dir(paths.parent())?;
        return Err(format!(
            "post-update health failed: {error}; previous binary restored"
        ));
    }
    durable_json(
        &paths.state,
        &InstalledState {
            schema_version: 1,
            version: to_version.into(),
            artifact_sha256: artifact_sha256.into(),
        },
    )?;
    hit(failpoint, "after-state-fsync")?;
    journal(
        paths,
        Phase::Committed,
        from_version,
        to_version,
        artifact_sha256,
        &previous_state,
    )?;
    if paths.backup.exists() {
        fs::remove_file(&paths.backup)
            .map_err(|error| format!("remove last-known-good after commit: {error}"))?;
    }
    fs::remove_file(&paths.journal)
        .map_err(|error| format!("remove committed journal: {error}"))?;
    sync_dir(paths.parent())?;
    Ok(())
}
