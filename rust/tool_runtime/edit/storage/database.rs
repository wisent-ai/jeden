//! Changing rows of a SQLite file under the workspace: one immediate
//! transaction per call, identifiers quoted rather than interpolated, and the
//! file's digest checked before and reported after.

use rusqlite::{params_from_iter, types::Value as SqlValue, Connection, TransactionBehavior};
use serde_json::{json, Value};

use super::file_sha;
use crate::tool_runtime::shared::{jail_write_path, string_input};
use crate::tool_runtime::ToolRuntime;

fn identifier(value: &str) -> Result<String, String> {
    if value.is_empty() || value.contains('\0') {
        return Err("invalid SQLite identifier".into());
    }
    Ok(format!("\"{}\"", value.replace('"', "\"\"")))
}

fn sql_value(value: &Value) -> Result<SqlValue, String> {
    match value {
        Value::Null => Ok(SqlValue::Null),
        Value::Bool(value) => Ok(SqlValue::Integer(i64::from(*value))),
        Value::Number(value) if value.is_i64() => {
            Ok(SqlValue::Integer(value.as_i64().unwrap_or_default()))
        }
        Value::Number(value) => value
            .as_f64()
            .map(SqlValue::Real)
            .ok_or("invalid SQLite number".into()),
        Value::String(value) => Ok(SqlValue::Text(value.clone())),
        other => Ok(SqlValue::Text(other.to_string())),
    }
}

pub(crate) fn write_sqlite(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    if !runtime.allow_write {
        return Err("write_sqlite requires --allow-write".into());
    }
    let label = string_input(input, "path").ok_or("write_sqlite requires path")?;
    let expected =
        string_input(input, "expectedSha256").ok_or("write_sqlite requires expectedSha256")?;
    let path = jail_write_path(runtime.cwd, &label)?;
    let (actual, _) = file_sha(&path)?;
    if actual != expected {
        return Err(format!(
            "expectedSha256 mismatch for {label}: expected {expected}, actual {actual}"
        ));
    }
    let table = string_input(input, "table").ok_or("write_sqlite requires table")?;
    let table_sql = identifier(&table)?;
    let action = string_input(input, "action").ok_or("write_sqlite requires action")?;
    let mut connection = Connection::open(&path).map_err(|error| error.to_string())?;
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())?;
    let affected = match action.as_str() {
        "insert" => {
            let row = input
                .get("row")
                .and_then(Value::as_object)
                .ok_or("SQLite insert requires row")?;
            if row.is_empty() {
                return Err("SQLite insert row is empty".into());
            }
            let columns = row
                .keys()
                .map(|key| identifier(key))
                .collect::<Result<Vec<_>, _>>()?;
            let values = row.values().map(sql_value).collect::<Result<Vec<_>, _>>()?;
            let marks = vec!["?"; values.len()].join(",");
            tx.execute(
                &format!(
                    "INSERT INTO {table_sql} ({}) VALUES ({marks})",
                    columns.join(",")
                ),
                params_from_iter(values),
            )
            .map_err(|error| error.to_string())?
        }
        "update" => {
            let row = input
                .get("row")
                .and_then(Value::as_object)
                .ok_or("SQLite update requires row")?;
            let key_column =
                string_input(input, "keyColumn").ok_or("SQLite update requires keyColumn")?;
            let key = input.get("key").ok_or("SQLite update requires key")?;
            if row.is_empty() {
                return Err("SQLite update row is empty".into());
            }
            let assignments = row
                .keys()
                .map(|name| identifier(name).map(|name| format!("{name} = ?")))
                .collect::<Result<Vec<_>, _>>()?;
            let mut values = row.values().map(sql_value).collect::<Result<Vec<_>, _>>()?;
            values.push(sql_value(key)?);
            tx.execute(
                &format!(
                    "UPDATE {table_sql} SET {} WHERE {} = ?",
                    assignments.join(","),
                    identifier(&key_column)?
                ),
                params_from_iter(values),
            )
            .map_err(|error| error.to_string())?
        }
        "delete" => {
            let key_column =
                string_input(input, "keyColumn").ok_or("SQLite delete requires keyColumn")?;
            let key = input.get("key").ok_or("SQLite delete requires key")?;
            tx.execute(
                &format!(
                    "DELETE FROM {table_sql} WHERE {} = ?",
                    identifier(&key_column)?
                ),
                [sql_value(key)?],
            )
            .map_err(|error| error.to_string())?
        }
        other => return Err(format!("unsupported SQLite action: {other}")),
    };
    tx.commit().map_err(|error| error.to_string())?;
    drop(connection);
    let (sha256, _) = file_sha(&path)?;
    Ok(
        json!({"ok":true,"path":label,"table":table,"action":action,"affected":affected,"sha256":sha256}),
    )
}
