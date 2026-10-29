use std::env;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::tool_runtime::runtime_ops::{
    ManagedCommand, OperationContext, ProcessManager, TerminationReason,
};

use super::super::EditorState;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ExternalEditorCommand {
    program: OsString,
    args: Vec<OsString>,
}

/// An editor variable: absent when unset, refused by name when not Unicode.
fn editor_variable(name: &str) -> Result<Option<String>, String> {
    match env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(value)) => Err(format!(
            "{name} is not Unicode ({value:?}); state the editor command as text"
        )),
    }
}

impl ExternalEditorCommand {
    fn from_environment() -> Result<Self, String> {
        let visual = editor_variable("VISUAL")?;
        let editor = editor_variable("EDITOR")?;
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
        // A program named with a directory is that file; a bare name is looked up on PATH.
        if program
            .parent()
            .is_some_and(|parent| !parent.as_os_str().is_empty())
        {
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
    let result = ProcessManager.run(operation, managed)?;
    if result.reason != TerminationReason::Completed {
        return Err(match result.reason {
            TerminationReason::Cancelled => "external editor cancelled".into(),
            TerminationReason::Completed => unreachable!(),
        });
    }
    if !result.status.success() {
        // An exit status without a code is a process a signal ended.
        return Err(match result.status.code() {
            Some(code) => format!("external editor exited with status {code}"),
            None => format!("external editor was ended by a signal ({})", result.status),
        });
    }

    let bytes = fs::read(temporary.path())
        .map_err(|error| format!("read external editor file: {error}"))?;
    let text = String::from_utf8(bytes)
        .map_err(|error| format!("external editor produced invalid UTF-8: {error}"))?;
    Ok(editor.replace_all_transaction(text))
}

struct TemporaryEditorFile {
    path: PathBuf,
    file: Option<File>,
}

impl TemporaryEditorFile {
    /// A new draft file under `root`, readable and writable by this user only,
    /// named so no other draft can hold the same name.
    fn create(root: &Path) -> Result<Self, String> {
        let path = root.join(format!("jeden-editor-{}.txt", uuid::Uuid::new_v4()));
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(u32::from(libc::S_IRUSR | libc::S_IWUSR));
        }
        let file = options
            .open(&path)
            .map_err(|error| format!("create external editor file {}: {error}", path.display()))?;
        Ok(Self {
            path,
            file: Some(file),
        })
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn write(&mut self, bytes: &[u8]) -> Result<(), String> {
        let Some(mut file) = self.file.take() else {
            return Err(format!(
                "the external editor file {} was already written and closed; it is written once",
                self.path.display()
            ));
        };
        file.write_all(bytes)
            .map_err(|error| format!("write external editor file: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("sync external editor file: {error}"))?;
        Ok(())
    }
}

impl Drop for TemporaryEditorFile {
    // The file holds the prompt the operator was writing; one that cannot be
    // removed is said with its path rather than left on disk unannounced.
    fn drop(&mut self) {
        if let Err(error) = fs::remove_file(&self.path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                eprintln!(
                    "jeden: the external editor draft {} is left behind: {error}",
                    self.path.display()
                );
            }
        }
    }
}

/// A file whose owner, group or others may execute it; a path that cannot be
/// read is not one this process can run.
#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let executable = u32::from(libc::S_IXUSR | libc::S_IXGRP | libc::S_IXOTH);
    match path.metadata() {
        Ok(metadata) => metadata.is_file() && metadata.permissions().mode() & executable != 0,
        Err(_unreadable) => false,
    }
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}
