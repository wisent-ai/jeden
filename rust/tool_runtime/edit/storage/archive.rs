//! Changing one entry of an archive in place: the whole archive is rewritten
//! beside the original and renamed over it, so a failed rewrite leaves the
//! caller's archive exactly as it was. The original is read from one open
//! handle, hashed and then rewritten as a stream, so an archive of any size
//! is changed without being held in memory, and the bytes rewritten are the
//! bytes whose digest was checked.

use flate2::{read::GzDecoder, write::GzEncoder, Compression};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Cursor, Read, Seek, Write};
use std::path::Path;
use zip::write::SimpleFileOptions;

use super::{file_sha, safe_entry};
use crate::tool_runtime::shared::{jail_write_path, string_input};
use crate::tool_runtime::ToolRuntime;

/// A writer that only feeds a digest, so a file can be hashed by copying it.
struct Hashing {
    digest: Sha256,
}

impl Write for Hashing {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.digest.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The refusal for one failed step of a rewrite, naming the step.
fn failed<'a, E: std::fmt::Display>(step: &'a str) -> impl FnOnce(E) -> String + 'a {
    move |error| format!("{step}: {error}")
}

fn rewrite_zip(
    source: File,
    target: &Path,
    entry_name: &str,
    content: Option<&[u8]>,
) -> Result<(), String> {
    let mut archive =
        zip::ZipArchive::new(source).map_err(failed("the archive cannot be read as a zip"))?;
    let output = File::create(target).map_err(|error| {
        format!("the rewritten archive {} cannot be created: {error}", target.display())
    })?;
    let mut writer = zip::ZipWriter::new(output);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for index in crate::tally::ORIGIN..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("zip entry number {index} cannot be read: {error}"))?;
        let name = entry.name().to_string();
        safe_entry(&name)?;
        if name == entry_name {
            continue;
        }
        if entry.is_dir() {
            writer
                .add_directory(name.as_str(), options)
                .map_err(|error| format!("zip directory {name} cannot be written: {error}"))?;
        } else {
            writer
                .start_file(name.as_str(), options)
                .map_err(|error| format!("zip entry {name} cannot be started: {error}"))?;
            std::io::copy(&mut entry, &mut writer)
                .map_err(|error| format!("zip entry {name} cannot be copied: {error}"))?;
        }
    }
    if let Some(bytes) = content {
        writer
            .start_file(entry_name, options)
            .map_err(|error| format!("zip entry {entry_name} cannot be started: {error}"))?;
        writer
            .write_all(bytes)
            .map_err(|error| format!("zip entry {entry_name} cannot be written: {error}"))?;
    }
    writer
        .finish()
        .map_err(failed("the rewritten zip cannot be finished"))?
        .sync_all()
        .map_err(failed("the rewritten zip cannot be synced to disk"))
}

fn rewrite_tar(
    source: File,
    target: &Path,
    entry_name: &str,
    content: Option<&[u8]>,
    gzip: bool,
) -> Result<(), String> {
    let reader: Box<dyn Read> = if gzip {
        Box::new(GzDecoder::new(source))
    } else {
        Box::new(source)
    };
    let output = File::create(target).map_err(|error| {
        format!("the rewritten archive {} cannot be created: {error}", target.display())
    })?;
    let sink: Box<dyn Write> = if gzip {
        Box::new(GzEncoder::new(output, Compression::default()))
    } else {
        Box::new(output)
    };
    let mut archive = tar::Archive::new(reader);
    let mut builder = tar::Builder::new(sink);
    for entry in archive
        .entries()
        .map_err(failed("the archive cannot be read as a tar"))?
    {
        let mut entry = entry.map_err(failed("a tar entry cannot be read"))?;
        let name = entry
            .path()
            .map_err(failed("a tar entry's path cannot be read"))?
            .to_string_lossy()
            .to_string();
        safe_entry(&name)?;
        if name == entry_name {
            continue;
        }
        let header = entry.header().clone();
        builder
            .append(&header, &mut entry)
            .map_err(|error| format!("tar entry {name} cannot be copied: {error}"))?;
    }
    if let Some(bytes) = content {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        // A written entry is a regular file its owner may change and anyone may read.
        header.set_mode(u32::from(libc::S_IRUSR | libc::S_IWUSR | libc::S_IRGRP | libc::S_IROTH));
        header.set_cksum();
        builder
            .append_data(&mut header, entry_name, Cursor::new(bytes))
            .map_err(|error| format!("tar entry {entry_name} cannot be written: {error}"))?;
    }
    builder
        .finish()
        .map_err(failed("the rewritten tar cannot be finished"))?;
    let mut sink = builder
        .into_inner()
        .map_err(failed("the rewritten tar cannot be closed"))?;
    sink.flush()
        .map_err(failed("the rewritten tar cannot be flushed"))
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
    let mut source = File::open(&path).map_err(|error| format!("{label} cannot be opened: {error}"))?;
    let mut hashing = Hashing { digest: Sha256::new() };
    std::io::copy(&mut source, &mut hashing)
        .map_err(|error| format!("{label} cannot be read to check its digest: {error}"))?;
    let actual = hex::encode(hashing.digest.finalize());
    if actual != expected {
        return Err(format!(
            "expectedSha256 mismatch for {label}: expected {expected}, actual {actual}"
        ));
    }
    source
        .rewind()
        .map_err(|error| format!("{label} cannot be read again after its digest: {error}"))?;
    let action =
        string_input(input, "action").ok_or("write_archive requires action: upsert or delete")?;
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
        rewrite_zip(source, &temp, &entry, content.as_deref())
    } else if lower.ends_with(".tar") {
        rewrite_tar(source, &temp, &entry, content.as_deref(), false)
    } else if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
        rewrite_tar(source, &temp, &entry, content.as_deref(), true)
    } else {
        Err("write_archive supports .zip, .tar, .tar.gz, and .tgz".into())
    };
    if let Err(error) = result {
        return Err(failed_leaving(format!("{label} cannot be rewritten: {error}"), &temp));
    }
    fs::rename(&temp, &path).map_err(|error| {
        failed_leaving(
            format!("the rewritten archive cannot replace {label}: {error}"),
            &temp,
        )
    })?;
    let (sha256, bytes) = file_sha(&path)?;
    Ok(json!({"ok":true,"path":label,"entry":entry,"action":action,"sha256":sha256,"bytes":bytes}))
}

/// `error`, followed by the sentence of the temporary archive a failed
/// rewrite could not remove; a temporary that is already gone is not a
/// failure.
fn failed_leaving(error: String, temporary: &Path) -> String {
    match fs::remove_file(temporary) {
        Err(left) if left.kind() != std::io::ErrorKind::NotFound => {
            format!("{error}; {} is left behind: {left}", temporary.display())
        }
        _ => error,
    }
}
