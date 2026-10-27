//! Writing a Jupyter notebook from the cell-marked text a read of it shows.

use serde_json::{json, Map, Value};
use std::fs;
use std::path::Path;

/// The notebook a write starts from when the file does not exist yet: no
/// cells, no metadata, in the notebook format version Jupyter writes.
const EMPTY_NOTEBOOK: &str = include_str!("empty.ipynb");

const CELL_MARKER: &str = "# %% [";

/// The bytes a write of `content` to `file` stores. Text for a `.ipynb` that
/// begins with a cell marker is rebuilt into the notebook's cells, keeping each
/// existing cell's metadata and outputs in place; anything else is stored as
/// it was given.
pub(super) fn notebook_bytes(file: &Path, content: &str) -> Result<Vec<u8>, String> {
    if file.extension().and_then(|value| value.to_str()) != Some("ipynb")
        || !content.starts_with(CELL_MARKER)
    {
        return Ok(content.as_bytes().to_vec());
    }
    let mut parsed = match fs::read(file) {
        Ok(bytes) => serde_json::from_slice::<Value>(&bytes)
            .map_err(|error| format!("{} is not a notebook: {error}", file.display()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            serde_json::from_str::<Value>(EMPTY_NOTEBOOK).expect("empty.ipynb is JSON")
        }
        Err(error) => return Err(format!("{} cannot be read: {error}", file.display())),
    };
    let cells = marked_cells(content)?;
    let existing = match parsed.get("cells") {
        Some(Value::Array(existing)) => existing.clone(),
        Some(other) => {
            return Err(format!(
                "{} holds cells that are not a list: {other}",
                file.display()
            ))
        }
        None => return Err(format!("{} holds no cells list", file.display())),
    };
    let rebuilt = cells
        .into_iter()
        .enumerate()
        .map(|(index, (kind, source))| {
            let mut cell = match existing.get(index) {
                Some(cell) => cell.clone(),
                None => json!({"metadata": Map::new()}),
            };
            let object = cell
                .as_object_mut()
                .ok_or("notebook cell must be an object")?;
            object.insert("cell_type".into(), json!(kind));
            object.insert(
                "source".into(),
                json!(source
                    .split_inclusive('\n')
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()),
            );
            if object.get("cell_type").and_then(Value::as_str) == Some("code") {
                object
                    .entry("outputs")
                    .or_insert_with(|| Value::Array(Vec::new()));
                object.entry("execution_count").or_insert(Value::Null);
            }
            Ok(cell)
        })
        .collect::<Result<Vec<_>, String>>()?;
    parsed
        .as_object_mut()
        .ok_or("notebook root must be an object")?
        .insert("cells".into(), json!(rebuilt));
    serde_json::to_vec_pretty(&parsed).map_err(|error| error.to_string())
}

/// Each `# %% [kind] cell:N` section of `content` as its kind and source.
fn marked_cells(content: &str) -> Result<Vec<(String, String)>, String> {
    let mut cells = Vec::<(String, String)>::new();
    let mut kind = None::<String>;
    let mut source = String::new();
    for line in content.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if let Some(rest) = trimmed.strip_prefix(CELL_MARKER) {
            if let Some((next_kind, suffix)) = rest.split_once("] cell:") {
                suffix.parse::<usize>().map_err(|error| {
                    format!("invalid notebook cell marker {trimmed:?}: its cell number {error}")
                })?;
                if let Some(previous) = kind.replace(next_kind.to_string()) {
                    cells.push((
                        previous,
                        std::mem::take(&mut source)
                            .trim_end_matches('\n')
                            .to_string(),
                    ));
                }
                continue;
            }
        }
        if kind.is_none() {
            return Err("notebook content must begin with a cell marker".into());
        }
        source.push_str(line);
    }
    if let Some(previous) = kind {
        cells.push((previous, source.trim_end_matches('\n').to_string()));
    }
    Ok(cells)
}
