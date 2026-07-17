use std::env;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::tool_runtime::runtime_ops::{
    ManagedCommand, OperationContext, ProcessManager, TerminationReason,
};

use super::super::EditorState;

const EXTERNAL_EDITOR_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);
static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ExternalEditorCommand {
    program: OsString,
    args: Vec<OsString>,
}

impl ExternalEditorCommand {
    fn from_environment() -> Result<Self, String> {
        let visual = env::var("VISUAL").ok();
        let editor = env::var("EDITOR").ok();
        Self::from_configuration(visual.as_deref(), editor.as_deref())
    }

    fn from_configuration(visual: Option<&str>, editor: Option<&str>) -> Result<Self, String> {
        let configured = visual
            .filter(|value| !value.trim().is_empty())
            .or_else(|| editor.filter(|value| !value.trim().is_empty()))
            .ok_or_else(|| "external editor unavailable: set VISUAL or EDITOR".to_string())?;
        Self::parse(configured)
    }

    fn parse(configured: &str) -> Result<Self, String> {
        let mut words = shell_words::split(configured)
            .map_err(|error| format!("invalid external editor command: {error}"))?
            .into_iter();
        let program = words
            .next()
            .filter(|program| !program.is_empty())
            .ok_or_else(|| "invalid external editor command: program is empty".to_string())?;
        Ok(Self {
            program: program.into(),
            args: words.map(OsString::from).collect(),
        })
    }

    fn is_available(&self, cwd: &Path) -> bool {
        let program = Path::new(&self.program);
        if program.components().count() > 1 {
            let resolved = if program.is_absolute() {
                program.to_path_buf()
            } else {
                cwd.join(program)
            };
            return is_executable(&resolved);
        }
        env::var_os("PATH")
            .into_iter()
            .flat_map(|path| env::split_paths(&path).collect::<Vec<_>>())
            .any(|directory| is_executable(&directory.join(program)))
    }
}

pub(crate) fn external_editor_health(cwd: &Path) -> Result<(), String> {
    let command = ExternalEditorCommand::from_environment()?;
    if command.is_available(cwd) {
        Ok(())
    } else {
        Err(format!(
            "external editor executable `{}` was not found",
            command.program.to_string_lossy()
        ))
    }
}

pub(crate) fn external_editor(
    editor: &mut EditorState,
    cwd: &Path,
    operation: &OperationContext<'_>,
) -> Result<bool, String> {
    let command = ExternalEditorCommand::from_environment()?;
    if !command.is_available(cwd) {
        return Err(format!(
            "external editor executable `{}` was not found",
            command.program.to_string_lossy()
        ));
    }
    external_editor_with(editor, cwd, operation, command, &env::temp_dir())
}

fn external_editor_with(
    editor: &mut EditorState,
    cwd: &Path,
    operation: &OperationContext<'_>,
    command: ExternalEditorCommand,
    temp_root: &Path,
) -> Result<bool, String> {
    if operation.cancellation().is_cancelled() {
        return Err("external editor cancelled".into());
    }
    let mut temporary = TemporaryEditorFile::create(temp_root)?;
    temporary.write(editor.text().as_bytes())?;

    let mut managed = ManagedCommand::new(command.program, cwd);
    managed.args = command.args;
    managed.args.push(temporary.path().as_os_str().to_owned());
    managed.inherit_stdio_for_foreground();
    let result = ProcessManager.run(operation, managed, EXTERNAL_EDITOR_TIMEOUT)?;
    if result.reason != TerminationReason::Completed {
        return Err(match result.reason {
            TerminationReason::Cancelled => "external editor cancelled".into(),
            TerminationReason::TimedOut => "external editor timed out".into(),
            TerminationReason::Completed => unreachable!(),
        });
    }
    if !result.status.success() {
        return Err(format!(
            "external editor exited with status {}",
            result
                .status
                .code()
                .map_or_else(|| "unknown".into(), |code| code.to_string())
        ));
    }

    let bytes = fs::read(temporary.path())
        .map_err(|error| format!("read external editor file: {error}"))?;
    let text = String::from_utf8(bytes)
        .map_err(|_| "external editor produced invalid UTF-8".to_string())?;
    editor
        .replace_all_transaction(text)
        .map_err(|error| error.to_string())
}

struct TemporaryEditorFile {
    path: PathBuf,
    file: Option<File>,
}

impl TemporaryEditorFile {
    fn create(root: &Path) -> Result<Self, String> {
        for _ in 0..128 {
            let id = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
            let path = root.join(format!("jeden-editor-{}-{id}.txt", std::process::id()));
            let mut options = OpenOptions::new();
            options.create_new(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(&path) {
                Ok(file) => {
                    return Ok(Self {
                        path,
                        file: Some(file),
                    })
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(format!("create external editor file: {error}")),
            }
        }
        Err("create external editor file: exhausted unique names".into())
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn write(&mut self, bytes: &[u8]) -> Result<(), String> {
        let mut file = self
            .file
            .take()
            .expect("temporary editor file is written once");
        file.write_all(bytes)
            .map_err(|error| format!("write external editor file: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("sync external editor file: {error}"))?;
        Ok(())
    }
}

impl Drop for TemporaryEditorFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}
