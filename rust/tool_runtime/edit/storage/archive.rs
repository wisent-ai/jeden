//! Changing one entry of an archive in place: the whole archive is rewritten
//! beside the original and renamed over it, so a failed rewrite leaves the
//! caller's archive exactly as it was.

use flate2::{read::GzDecoder, write::GzEncoder, Compression};
use serde_json::{json, Value};
use std::fs::{self, File};
use std::io::{Cursor, Read, Write};
use std::path::Path;
use zip::write::SimpleFileOptions;

use super::{file_sha, safe_entry, MAX_ARCHIVE_WRITE_BYTES};
use crate::tool_runtime::shared::{jail_write_path, string_input, verify_expected_sha};
use crate::tool_runtime::ToolRuntime;

fn rewrite_zip(
    source: &[u8],
    target: &Path,
    entry_name: &str,
    content: Option<&[u8]>,
) -> Result<(), String> {
    let mut archive =
        zip::ZipArchive::new(Cursor::new(source)).map_err(|error| error.to_string())?;
    let output = File::create(target).map_err(|error| error.to_string())?;
    let mut writer = zip::ZipWriter::new(output);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|error| error.to_string())?;
        let name = entry.name().to_string();
        safe_entry(&name)?;
        if name == entry_name {
            continue;
        }
        if entry.is_dir() {
            writer
                .add_directory(name, options)
                .map_err(|error| error.to_string())?;
        } else {
            writer
                .start_file(name, options)
                .map_err(|error| error.to_string())?;
            std::io::copy(&mut entry, &mut writer).map_err(|error| error.to_string())?;
        }
    }
    if let Some(bytes) = content {
        writer
            .start_file(entry_name, options)
            .map_err(|error| error.to_string())?;
        writer.write_all(bytes).map_err(|error| error.to_string())?;
    }
    writer
        .finish()
        .map_err(|error| error.to_string())?
        .sync_all()
        .map_err(|error| error.to_string())
}

fn rewrite_tar(
    source: &[u8],
    target: &Path,
    entry_name: &str,
    content: Option<&[u8]>,
    gzip: bool,
) -> Result<(), String> {
    let reader: Box<dyn Read> = if gzip {
        Box::new(GzDecoder::new(Cursor::new(source)))
    } else {
        Box::new(Cursor::new(source))
    };
    let output = File::create(target).map_err(|error| error.to_string())?;
    let sink: Box<dyn Write> = if gzip {
        Box::new(GzEncoder::new(output, Compression::default()))
    } else {
        Box::new(output)
    };
    let mut archive = tar::Archive::new(reader);
    let mut builder = tar::Builder::new(sink);
    for entry in archive.entries().map_err(|error| error.to_string())? {
        let mut entry = entry.map_err(|error| error.to_string())?;
        let name = entry
            .path()
            .map_err(|error| error.to_string())?
            .to_string_lossy()
            .to_string();
        safe_entry(&name)?;
        if name == entry_name {
            continue;
        }
        let header = entry.header().clone();
        builder
            .append(&header, &mut entry)
            .map_err(|error| error.to_string())?;
    }
    if let Some(bytes) = content {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, entry_name, Cursor::new(bytes))
            .map_err(|error| error.to_string())?;
    }
    builder.finish().map_err(|error| error.to_string())?;
    let mut sink = builder.into_inner().map_err(|error| error.to_string())?;
    sink.flush().map_err(|error| error.to_string())
}

pub(crate) fn write_archive(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    if !runtime.allow_write {
        return Err("write_archive requires --allow-write".into());
    }
    let label = string_input(input, "path").ok_or("write_archive requires path")?;
    let entry = string_input(input, "entry").ok_or("write_archive requires entry")?;
    safe_entry(&entry)?;
    let expected =
        string_input(input, "expectedSha256").ok_or("write_archive requires expectedSha256")?;
    let path = jail_write_path(runtime.cwd, &label)?;
    let metadata = fs::metadata(&path).map_err(|error| error.to_string())?;
    if metadata.len() > MAX_ARCHIVE_WRITE_BYTES {
        return Err(format!(
            "archive exceeds write limit of {MAX_ARCHIVE_WRITE_BYTES} bytes"
        ));
    }
    let source = verify_expected_sha(&label, &path, &expected)?;
    let action = string_input(input, "action").unwrap_or_else(|| "upsert".into());
    let content = match action.as_str() {
        "upsert" => Some(
            string_input(input, "content")
                .ok_or("archive upsert requires content")?
                .into_bytes(),
        ),
        "delete" => None,
        other => return Err(format!("unsupported archive action: {other}")),
    };
    let temp = path.with_extension(format!("jeden-{}.tmp", std::process::id()));
    let lower = label.to_ascii_lowercase();
    let result = if lower.ends_with(".zip") {
        rewrite_zip(&source, &temp, &entry, content.as_deref())
    } else if lower.ends_with(".tar") {
        rewrite_tar(&source, &temp, &entry, content.as_deref(), false)
    } else if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
        rewrite_tar(&source, &temp, &entry, content.as_deref(), true)
    } else {
        Err("write_archive supports .zip, .tar, .tar.gz, and .tgz".into())
    };
    if let Err(error) = result {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    fs::rename(&temp, &path).map_err(|error| {
        let _ = fs::remove_file(&temp);
        error.to_string()
    })?;
    let (sha256, bytes) = file_sha(&path)?;
    Ok(json!({"ok":true,"path":label,"entry":entry,"action":action,"sha256":sha256,"bytes":bytes}))
}
