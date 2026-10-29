//! What a file-changing tool will leave, presented to policy hooks in the
//! shared anchored-section form (`[path#TAG]` with `PUT`, `CUT`, `REM` and
//! `MV`) that Tama's write gates rebuild and judge. Each section is computed
//! from the same plan the tool executes, before anything is written, so a
//! hook judges the complete file the tool leaves under the path it leaves it
//! at, and a removal or rename is judged as one rather than as a write.
use serde_json::{json, Value};
use std::fs;
use std::path::Path;

use super::lines::apply_line_edit_ops;
use super::visual_apply::plan_visual_edit;
use crate::tally::STEP;
use crate::tool_runtime::shared::{
    jail_write_path, sha256_hex, snapshot_tag, split_edit_lines, string_input,
};

/// Line numbers count from one step past none.
const FIRST_LINE: usize = STEP as usize;

/// One file's change as a policy hook reads it.
struct Section {
    path: String,
    tag: String,
    operations: Vec<String>,
    added: Vec<String>,
}

impl Section {
    fn new(path: &str, before: &[u8]) -> Self {
        Self {
            path: path.to_owned(),
            tag: snapshot_tag(&sha256_hex(before)),
            operations: Vec::new(),
            added: Vec::new(),
        }
    }

    /// The single hunk that turns `before` into `after`: the lines between
    /// their common head and common tail, replaced, cut or inserted, every
    /// number counted in `before` as it is before the call.
    fn changed(mut self, before: &str, after: &str) -> Self {
        let (old, _) = split_edit_lines(before);
        let (new, _) = split_edit_lines(after);
        let head = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
        let tail = old[head..]
            .iter()
            .rev()
            .zip(new[head..].iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        let removed = &old[head..old.len() - tail];
        let added = &new[head..new.len() - tail];
        let first = head + FIRST_LINE;
        let last = old.len() - tail;
        match (removed.is_empty(), added.is_empty()) {
            (true, true) => return self,
            (false, true) => self.operations.push(format!("CUT {first}.={last}")),
            (false, false) => self.operations.push(format!("PUT {first}.={last}:")),
            (true, false) if head == old.len() && !old.is_empty() => {
                self.operations.push(format!("PUT >{head}:"))
            }
            (true, false) => self.operations.push(format!("PUT <{first}:")),
        }
        self.operations
            .extend(added.iter().map(|line| format!("+{line}")));
        self.added.extend(added.iter().cloned());
        self
    }

    fn moved(mut self, destination: &str) -> Self {
        self.operations.push(format!("MV {}", quoted(destination)));
        self
    }

    fn removed(mut self) -> Self {
        self.operations.push("REM".to_owned());
        self
    }

    fn input(self) -> Value {
        let patch = format!(
            "*** Begin Patch\n[{}#{}]\n{}\n*** End Patch\n",
            self.path,
            self.tag,
            self.operations.join("\n")
        );
        let mut input = json!({ "path": self.path, "patch": patch });
        if !self.added.is_empty() {
            input["new_string"] = json!(self.added.join("\n"));
        }
        input
    }
}

/// A destination as one literal word: bare when it holds no space, quote or
/// escape, otherwise single-quoted as POSIX quoting reads it.
fn quoted(destination: &str) -> String {
    let plain = !destination.is_empty()
        && !destination.contains(|ch: char| ch.is_whitespace() || matches!(ch, '\'' | '"' | '\\'));
    if plain {
        destination.to_owned()
    } else {
        format!("'{}'", destination.replace('\'', r#"'"'"'"#))
    }
}

fn read_existing(cwd: &Path, path: &str) -> Result<Vec<u8>, String> {
    let file = jail_write_path(cwd, path)?;
    fs::read(&file).map_err(|cause| format!("cannot read {}: {cause}", file.display()))
}

fn text(path: &str, bytes: &[u8]) -> Result<String, String> {
    String::from_utf8(bytes.to_vec()).map_err(|cause| format!("{path} is not UTF-8 text: {cause}"))
}

fn required<'a>(input: &'a Value, tool: &str, key: &str) -> Result<String, String> {
    string_input(input, key).ok_or_else(|| format!("{tool} requires {key}"))
}

/// The sections a policy hook judges for `tool`, or nothing for a tool that
/// already names its file and complete content, or changes no file. A call
/// whose change cannot be planned is the error: its hooks would otherwise be
/// handed a request they cannot read and judge nothing.
pub(crate) fn policy_subjects(
    cwd: &Path,
    tool: &str,
    input: &Value,
) -> Result<Option<Vec<Value>>, String> {
    let sections = match tool {
        "edit" => plan_visual_edit(cwd, &required(input, tool, "patch")?)?
            .into_iter()
            .map(|item| {
                let section = Section::new(&item.path, item.before.as_bytes());
                let section = if item.remove {
                    section.removed()
                } else {
                    section.changed(&item.before, &item.after)
                };
                match &item.to_path {
                    Some(destination) => section.moved(destination),
                    None => section,
                }
            })
            .collect(),
        "edit_file" => {
            let path = required(input, tool, "path")?;
            let bytes = read_existing(cwd, &path)?;
            let before = text(&path, &bytes)?;
            let ops = input.get("ops").ok_or("ops are required")?;
            let after = apply_line_edit_ops(&before, ops)?;
            vec![Section::new(&path, &bytes).changed(&before, &after)]
        }
        "apply_patch" => {
            let path = required(input, tool, "path")?;
            let bytes = read_existing(cwd, &path)?;
            let before = text(&path, &bytes)?;
            let replacements = input
                .get("replacements")
                .ok_or("replacements are required")?;
            let after = super::replaced(&before, replacements)?;
            vec![Section::new(&path, &bytes).changed(&before, &after)]
        }
        "delete_file" => {
            let path = required(input, tool, "path")?;
            vec![Section::new(&path, &read_existing(cwd, &path)?).removed()]
        }
        "move_file" => {
            let from = required(input, tool, "from")?;
            let to = required(input, tool, "to")?;
            vec![Section::new(&from, &read_existing(cwd, &from)?).moved(&to)]
        }
        _ => return Ok(None),
    };
    Ok(Some(sections.into_iter().map(Section::input).collect()))
}
