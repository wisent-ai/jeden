use super::{VisualPatchOp, VisualPatchSection};

fn parse_visual_body(lines: &[&str], mut index: usize) -> Result<(Vec<String>, usize), String> {
    let mut body = Vec::new();
    while index < lines.len() {
        let line = lines[index];
        if line == "*** End Patch"
            || (line.starts_with('[') && line.ends_with(']') && line.contains('#'))
            || line.starts_with("SWAP ")
            || line.starts_with("SWAP.BLK ")
            || line.starts_with("DEL ")
            || line.starts_with("DEL.BLK ")
            || line.starts_with("INS.")
            || line == "REM"
            || line.starts_with("MV ")
        {
            break;
        }
        let Some(rest) = line.strip_prefix('+') else { return Err(format!("patch body line must start with +: {line}")); };
        body.push(rest.to_string());
        index += 1;
    }
    Ok((body, index))
}

pub(super) fn parse_visual_edit_patch(patch: &str) -> Result<Vec<VisualPatchSection>, String> {
    if patch.trim().is_empty() { return Err("patch is required".into()); }
    let normalized = patch.replace("\r\n", "\n");
    let mut lines = normalized.split('\n').collect::<Vec<_>>();
    while lines.last() == Some(&"") { lines.pop(); }
    if lines.first() != Some(&"*** Begin Patch") { return Err("patch must start with *** Begin Patch".into()); }
    if lines.last() != Some(&"*** End Patch") { return Err("patch must end with *** End Patch".into()); }
    let mut sections: Vec<VisualPatchSection> = Vec::new();
    let mut index = 1usize;
    while index < lines.len() - 1 {
        let line = lines[index];
        if line.trim().is_empty() {
            index += 1;
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            let inner = &line[1..line.len() - 1];
            let Some((path, tag)) = inner.rsplit_once('#') else { return Err(format!("invalid file header: {line}")); };
            sections.push(VisualPatchSection { path: path.to_string(), tag: tag.to_ascii_uppercase(), ops: Vec::new(), remove: false, move_to: None });
            index += 1;
            continue;
        }
        let Some(current) = sections.last_mut() else { return Err("patch hunk appears before file header".into()); };
        if let Some(rest) = line.strip_prefix("SWAP.BLK ") {
            let line_no = rest.strip_suffix(':').ok_or_else(|| format!("unsupported patch line: {line}"))?.parse::<usize>().map_err(|_| format!("unsupported patch line: {line}"))?;
            let (content, next) = parse_visual_body(&lines, index + 1)?;
            if content.is_empty() { return Err("SWAP.BLK hunk requires at least one + body line; use DEL.BLK to delete".into()); }
            current.ops.push(VisualPatchOp { op: "replace_block".into(), start_line: None, end_line: None, line: Some(line_no), content });
            index = next;
            continue;
        }
        if let Some(rest) = line.strip_prefix("DEL.BLK ") {
            let line_no = rest.parse::<usize>().map_err(|_| format!("unsupported patch line: {line}"))?;
            current.ops.push(VisualPatchOp { op: "delete_block".into(), start_line: None, end_line: None, line: Some(line_no), content: Vec::new() });
            index += 1;
            continue;
        }
        if let Some(rest) = line.strip_prefix("INS.BLK.POST ") {
            let line_no = rest.strip_suffix(':').ok_or_else(|| format!("unsupported patch line: {line}"))?.parse::<usize>().map_err(|_| format!("unsupported patch line: {line}"))?;
            let (content, next) = parse_visual_body(&lines, index + 1)?;
            if content.is_empty() { return Err("INS.BLK.POST hunk requires at least one + body line".into()); }
            current.ops.push(VisualPatchOp { op: "insert_block_after".into(), start_line: None, end_line: None, line: Some(line_no), content });
            index = next;
            continue;
        }
        if line == "REM" {
            current.remove = true;
            index += 1;
            continue;
        }
        if let Some(dest) = line.strip_prefix("MV ") {
            current.move_to = Some(dest.trim().trim_matches('"').to_string());
            index += 1;
            continue;
        }
        if let Some(rest) = line.strip_prefix("SWAP ") {
            let range = rest.strip_suffix(':').ok_or_else(|| format!("unsupported patch line: {line}"))?;
            let Some((start, end)) = range.split_once(".=") else { return Err(format!("unsupported patch line: {line}")); };
            let start = start.parse::<usize>().map_err(|_| format!("unsupported patch line: {line}"))?;
            let end = end.parse::<usize>().map_err(|_| format!("unsupported patch line: {line}"))?;
            let (content, next) = parse_visual_body(&lines, index + 1)?;
            if content.is_empty() { return Err("SWAP hunk requires at least one + body line; use DEL to delete".into()); }
            current.ops.push(VisualPatchOp { op: "replace".into(), start_line: Some(start), end_line: Some(end), line: None, content });
            index = next;
            continue;
        }
        if let Some(rest) = line.strip_prefix("DEL ") {
            let (start, end) = if let Some((start, end)) = rest.split_once(".=") {
                (start.parse::<usize>().map_err(|_| format!("unsupported patch line: {line}"))?, end.parse::<usize>().map_err(|_| format!("unsupported patch line: {line}"))?)
            } else {
                let start = rest.parse::<usize>().map_err(|_| format!("unsupported patch line: {line}"))?;
                (start, start)
            };
            current.ops.push(VisualPatchOp { op: "delete".into(), start_line: Some(start), end_line: Some(end), line: None, content: Vec::new() });
            index += 1;
            continue;
        }
        if let Some(rest) = line.strip_prefix("INS.PRE ") {
            let line_no = rest.strip_suffix(':').ok_or_else(|| format!("unsupported patch line: {line}"))?.parse::<usize>().map_err(|_| format!("unsupported patch line: {line}"))?;
            let (content, next) = parse_visual_body(&lines, index + 1)?;
            if content.is_empty() { return Err("INS hunk requires at least one + body line".into()); }
            current.ops.push(VisualPatchOp { op: "insert_before".into(), start_line: None, end_line: None, line: Some(line_no), content });
            index = next;
            continue;
        }
        if let Some(rest) = line.strip_prefix("INS.POST ") {
            let line_no = rest.strip_suffix(':').ok_or_else(|| format!("unsupported patch line: {line}"))?.parse::<usize>().map_err(|_| format!("unsupported patch line: {line}"))?;
            let (content, next) = parse_visual_body(&lines, index + 1)?;
            if content.is_empty() { return Err("INS hunk requires at least one + body line".into()); }
            current.ops.push(VisualPatchOp { op: "insert_after".into(), start_line: None, end_line: None, line: Some(line_no), content });
            index = next;
            continue;
        }
        if line == "INS.HEAD:" || line == "INS.TAIL:" {
            let (content, next) = parse_visual_body(&lines, index + 1)?;
            if content.is_empty() { return Err("INS hunk requires at least one + body line".into()); }
            current.ops.push(VisualPatchOp { op: if line == "INS.HEAD:" { "insert_head".into() } else { "insert_tail".into() }, start_line: None, end_line: None, line: None, content });
            index = next;
            continue;
        }
        return Err(format!("unsupported patch line: {line}"));
    }
    if sections.is_empty() { return Err("patch must include at least one file section".into()); }
    for section in &sections {
        if section.remove && (!section.ops.is_empty() || section.move_to.is_some()) { return Err(format!("REM cannot be combined with other hunks: {}", section.path)); }
        if section.ops.is_empty() && !section.remove && section.move_to.is_none() { return Err(format!("patch file section has no hunks: {}", section.path)); }
    }
    Ok(sections)
}
