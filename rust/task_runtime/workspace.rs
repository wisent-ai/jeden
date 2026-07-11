use super::types::TaskError;
use crate::tool_runtime::runtime_ops::platform::native;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct IsolatedWorkspace {
    pub path: PathBuf,
    pub strategy: String,
    pub(crate) parent: PathBuf,
}

pub fn isolate(parent: &Path, root: &Path, id: &str) -> Result<IsolatedWorkspace, TaskError> {
    fs::create_dir_all(root)?;
    let target = root.join(id);
    if target.exists() {
        return Err(TaskError::Conflict(format!(
            "workspace already exists: {}",
            target.display()
        )));
    }
    let strategy = native()
        .isolate(parent, &target)
        .map_err(|error| TaskError::Io(error.to_string()))?;
    Ok(IsolatedWorkspace {
        path: target,
        strategy: strategy.into(),
        parent: parent.into(),
    })
}

impl IsolatedWorkspace {
    pub fn capture(&self, destination: &Path, max_bytes: u64) -> Result<(), TaskError> {
        let snapshot = native()
            .snapshot(&self.parent, &self.path, max_bytes)
            .map_err(|error| {
                let message = error.to_string();
                if message.contains("exceeds") {
                    TaskError::Capacity {
                        running: max_bytes.saturating_add(1) as usize,
                        limit: max_bytes as usize,
                    }
                } else {
                    TaskError::Process(message)
                }
            })?;
        fs::write(destination, snapshot)?;
        Ok(())
    }
    pub fn merge(&self, capture: &Path) -> Result<(), TaskError> {
        if !capture.exists() {
            return Err(TaskError::NotFound(format!(
                "capture not found: {}",
                capture.display()
            )));
        }
        let data = fs::read(capture)?;
        native()
            .apply_snapshot(&self.parent, &data, 64 * 1024)
            .map_err(|error| TaskError::Conflict(error.to_string()))
    }
}
