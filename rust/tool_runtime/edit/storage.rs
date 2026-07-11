use flate2::{read::GzDecoder, write::GzEncoder, Compression};
use rusqlite::{params_from_iter, types::Value as SqlValue, Connection, TransactionBehavior};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Cursor, Read, Write};
use std::path::Path;
use zip::write::SimpleFileOptions;

use crate::tool_runtime::shared::{jail_path, sha256_hex, string_input, verify_expected_sha};
use crate::tool_runtime::ToolRuntime;

const MAX_ARCHIVE_WRITE_BYTES:u64=128*1024*1024;

fn file_sha(path:&Path)->Result<(String,u64),String>{
    let mut file=File::open(path).map_err(|error|error.to_string())?;let mut hash=Sha256::new();let mut bytes=0u64;let mut buffer=[0u8;64*1024];
    loop{let count=file.read(&mut buffer).map_err(|error|error.to_string())?;if count==0{break;}hash.update(&buffer[..count]);bytes+=count as u64;}
    Ok((hex::encode(hash.finalize()),bytes))
}

fn safe_entry(name: &str) -> Result<(), String> {
    let path = Path::new(name);
    if name.is_empty() || path.is_absolute() || path.components().any(|component| matches!(component, std::path::Component::ParentDir)) {
        return Err(format!("unsafe archive entry path: {name}"));
    }
    Ok(())
}

fn rewrite_zip(source: &[u8], target: &Path, entry_name: &str, content: Option<&[u8]>) -> Result<(), String> {
    let mut archive = zip::ZipArchive::new(Cursor::new(source)).map_err(|error| error.to_string())?;
    let output = File::create(target).map_err(|error| error.to_string())?;
    let mut writer = zip::ZipWriter::new(output);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|error| error.to_string())?;
        let name = entry.name().to_string();
        safe_entry(&name)?;
        if name == entry_name { continue; }
        if entry.is_dir() { writer.add_directory(name, options).map_err(|error| error.to_string())?; }
        else { writer.start_file(name, options).map_err(|error| error.to_string())?; std::io::copy(&mut entry, &mut writer).map_err(|error| error.to_string())?; }
    }
    if let Some(bytes) = content { writer.start_file(entry_name, options).map_err(|error| error.to_string())?; writer.write_all(bytes).map_err(|error| error.to_string())?; }
    writer.finish().map_err(|error| error.to_string())?.sync_all().map_err(|error| error.to_string())
}

fn rewrite_tar(source: &[u8], target: &Path, entry_name: &str, content: Option<&[u8]>, gzip: bool) -> Result<(), String> {
    let reader: Box<dyn Read> = if gzip { Box::new(GzDecoder::new(Cursor::new(source))) } else { Box::new(Cursor::new(source)) };
    let output = File::create(target).map_err(|error| error.to_string())?;
    let sink: Box<dyn Write> = if gzip { Box::new(GzEncoder::new(output, Compression::default())) } else { Box::new(output) };
    let mut archive = tar::Archive::new(reader);
    let mut builder = tar::Builder::new(sink);
    for entry in archive.entries().map_err(|error| error.to_string())? {
        let mut entry = entry.map_err(|error| error.to_string())?;
        let name = entry.path().map_err(|error| error.to_string())?.to_string_lossy().to_string();
        safe_entry(&name)?;
        if name == entry_name { continue; }
        let header = entry.header().clone();
        builder.append(&header, &mut entry).map_err(|error| error.to_string())?;
    }
    if let Some(bytes) = content {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64); header.set_mode(0o644); header.set_cksum();
        builder.append_data(&mut header, entry_name, Cursor::new(bytes)).map_err(|error| error.to_string())?;
    }
    builder.finish().map_err(|error| error.to_string())?;
    let mut sink = builder.into_inner().map_err(|error| error.to_string())?;
    sink.flush().map_err(|error| error.to_string())
}

pub(crate) fn write_archive(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    if !runtime.allow_write { return Err("write_archive requires --allow-write".into()); }
    let label = string_input(input, "path").ok_or("write_archive requires path")?;
    let entry = string_input(input, "entry").ok_or("write_archive requires entry")?;
    safe_entry(&entry)?;
    let expected = string_input(input, "expectedSha256").ok_or("write_archive requires expectedSha256")?;
    let path = jail_path(runtime.cwd, &label)?;
    let metadata=fs::metadata(&path).map_err(|error|error.to_string())?;
    if metadata.len()>MAX_ARCHIVE_WRITE_BYTES{return Err(format!("archive exceeds write limit of {MAX_ARCHIVE_WRITE_BYTES} bytes"));}
    let source = verify_expected_sha(&label, &path, &expected)?;
    let action = string_input(input, "action").unwrap_or_else(|| "upsert".into());
    let content = match action.as_str() { "upsert" => Some(string_input(input,"content").ok_or("archive upsert requires content")?.into_bytes()), "delete" => None, other => return Err(format!("unsupported archive action: {other}")) };
    let temp = path.with_extension(format!("jeden-{}.tmp", std::process::id()));
    let lower = label.to_ascii_lowercase();
    let result = if lower.ends_with(".zip") { rewrite_zip(&source,&temp,&entry,content.as_deref()) } else if lower.ends_with(".tar") { rewrite_tar(&source,&temp,&entry,content.as_deref(),false) } else if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") { rewrite_tar(&source,&temp,&entry,content.as_deref(),true) } else { Err("write_archive supports .zip, .tar, .tar.gz, and .tgz".into()) };
    if let Err(error)=result { let _=fs::remove_file(&temp); return Err(error); }
    fs::rename(&temp,&path).map_err(|error| { let _=fs::remove_file(&temp); error.to_string() })?;
    let (sha256,bytes)=file_sha(&path)?;
    Ok(json!({"ok":true,"path":label,"entry":entry,"action":action,"sha256":sha256,"bytes":bytes}))
}

fn identifier(value: &str) -> Result<String, String> {
    if value.is_empty() || value.contains('\0') { return Err("invalid SQLite identifier".into()); }
    Ok(format!("\"{}\"",value.replace('"',"\"\"")))
}

fn sql_value(value: &Value) -> Result<SqlValue, String> {
    match value { Value::Null=>Ok(SqlValue::Null),Value::Bool(value)=>Ok(SqlValue::Integer(i64::from(*value))),Value::Number(value) if value.is_i64()=>Ok(SqlValue::Integer(value.as_i64().unwrap_or_default())),Value::Number(value)=>value.as_f64().map(SqlValue::Real).ok_or("invalid SQLite number".into()),Value::String(value)=>Ok(SqlValue::Text(value.clone())),other=>Ok(SqlValue::Text(other.to_string())) }
}

pub(crate) fn write_sqlite(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    if !runtime.allow_write { return Err("write_sqlite requires --allow-write".into()); }
    let label=string_input(input,"path").ok_or("write_sqlite requires path")?;
    let expected=string_input(input,"expectedSha256").ok_or("write_sqlite requires expectedSha256")?;
    let path=jail_path(runtime.cwd,&label)?;
    let (actual,_)=file_sha(&path)?;if actual!=expected{return Err(format!("expectedSha256 mismatch for {label}: expected {expected}, actual {actual}"));}
    let table=string_input(input,"table").ok_or("write_sqlite requires table")?;
    let table_sql=identifier(&table)?;
    let action=string_input(input,"action").ok_or("write_sqlite requires action")?;
    let mut connection=Connection::open(&path).map_err(|error| error.to_string())?;
    let tx=connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|error| error.to_string())?;
    let affected=match action.as_str() {
        "insert" => { let row=input.get("row").and_then(Value::as_object).ok_or("SQLite insert requires row")?; if row.is_empty(){return Err("SQLite insert row is empty".into());} let columns=row.keys().map(|key|identifier(key)).collect::<Result<Vec<_>,_>>()?; let values=row.values().map(sql_value).collect::<Result<Vec<_>,_>>()?; let marks=vec!["?";values.len()].join(","); tx.execute(&format!("INSERT INTO {table_sql} ({}) VALUES ({marks})",columns.join(",")),params_from_iter(values)).map_err(|error| error.to_string())? },
        "update" => { let row=input.get("row").and_then(Value::as_object).ok_or("SQLite update requires row")?; let key_column=string_input(input,"keyColumn").ok_or("SQLite update requires keyColumn")?; let key=input.get("key").ok_or("SQLite update requires key")?; if row.is_empty(){return Err("SQLite update row is empty".into());} let assignments=row.keys().map(|name|identifier(name).map(|name|format!("{name} = ?"))).collect::<Result<Vec<_>,_>>()?; let mut values=row.values().map(sql_value).collect::<Result<Vec<_>,_>>()?; values.push(sql_value(key)?); tx.execute(&format!("UPDATE {table_sql} SET {} WHERE {} = ?",assignments.join(","),identifier(&key_column)?),params_from_iter(values)).map_err(|error| error.to_string())? },
        "delete" => { let key_column=string_input(input,"keyColumn").ok_or("SQLite delete requires keyColumn")?; let key=input.get("key").ok_or("SQLite delete requires key")?; tx.execute(&format!("DELETE FROM {table_sql} WHERE {} = ?",identifier(&key_column)?),[sql_value(key)?]).map_err(|error| error.to_string())? },
        other=>return Err(format!("unsupported SQLite action: {other}")),
    };
    tx.commit().map_err(|error| error.to_string())?;
    drop(connection);
    let (sha256,_)=file_sha(&path)?;
    Ok(json!({"ok":true,"path":label,"table":table,"action":action,"affected":affected,"sha256":sha256}))
}
