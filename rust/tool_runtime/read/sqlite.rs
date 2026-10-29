use base64::{engine::general_purpose, Engine as _};
use rusqlite::{types::ValueRef as SqlValueRef, Connection};
use serde_json::{json, Value};

use crate::tool_runtime::shared::{amount_input, count_input, jail_path, string_input};
use crate::tool_runtime::ToolRuntime;

fn sql_json_value(value: SqlValueRef<'_>) -> Value {
    match value {
        SqlValueRef::Null => Value::Null,
        SqlValueRef::Integer(n) => json!(n),
        SqlValueRef::Real(n) => json!(n),
        SqlValueRef::Text(bytes) => json!(String::from_utf8_lossy(bytes).to_string()),
        SqlValueRef::Blob(bytes) => {
            json!({"base64": general_purpose::STANDARD.encode(bytes), "bytes": bytes.len()})
        }
    }
}

fn run_sql_rows(conn: &Connection, sql: &str) -> Result<Vec<Value>, String> {
    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
    let names = stmt
        .column_names()
        .iter()
        .map(|name| name.to_string())
        .collect::<Vec<_>>();
    let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        let mut object = serde_json::Map::new();
        for (idx, name) in names.iter().enumerate() {
            object.insert(
                name.clone(),
                sql_json_value(row.get_ref(idx).map_err(|e| e.to_string())?),
            );
        }
        out.push(Value::Object(object));
    }
    Ok(out)
}

fn sqlite_identifier(name: &str) -> Result<String, String> {
    if name.is_empty() || name.contains('\0') {
        return Err(format!("invalid SQLite identifier: {name}"));
    }
    Ok(format!("\"{}\"", name.replace('"', "\"\"")))
}

pub(crate) fn read_sqlite(runtime: &ToolRuntime<'_>, input: &Value) -> Result<Value, String> {
    let path = string_input(input, "path").ok_or("read_sqlite requires path")?;
    let file = jail_path(runtime.cwd, &path)?;
    let offset = amount_input(input, "offset", "read_sqlite")?.0;
    let conn = Connection::open_with_flags(&file, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| e.to_string())?;
    if let Some(query) = string_input(input, "query") {
        let query = query.trim().trim_end_matches(';').to_string();
        let lower = query.to_lowercase();
        if query.contains(';')
            || query.contains('\0')
            || !(lower.starts_with("select") || lower.starts_with("with"))
        {
            return Err("read_sqlite query must be a single SELECT or WITH statement".into());
        }
        let limit = count_input(input, "limit", "read_sqlite with a query")?;
        let rows = run_sql_rows(
            &conn,
            &format!("SELECT * FROM ({query}) LIMIT {limit} OFFSET {offset}"),
        )?;
        return Ok(
            json!({"path": path, "query": query, "rows": rows, "limit": limit, "offset": offset}),
        );
    }
    let table_name = match string_input(input, "table") {
        Some(table_name) => table_name,
        None => {
            let mut tables = run_sql_rows(&conn, "SELECT name, type FROM sqlite_schema WHERE type IN ('table','view') AND name NOT LIKE 'sqlite_%' ORDER BY name")?;
            for table in &mut tables {
                if table.get("type").and_then(Value::as_str) == Some("table") {
                    if let Some(name) = table.get("name").and_then(Value::as_str) {
                        let rows: i64 = conn
                            .query_row(
                                &format!(
                                    "SELECT count(*) AS count FROM {}",
                                    sqlite_identifier(name)?
                                ),
                                [],
                                |row| row.get("count"),
                            )
                            .map_err(|error| format!("cannot count the rows of {name}: {error}"))?;
                        table["rows"] = json!(rows);
                    }
                }
            }
            return Ok(json!({"path": path, "tables": tables}));
        }
    };
    let table_sql = sqlite_identifier(&table_name)?;
    let schema = run_sql_rows(&conn, &format!("PRAGMA table_info({table_sql})"))?;
    if let Some(key) = string_input(input, "key") {
        // `pk` is a column's place in the primary key, counted from one; a
        // column outside the key has none.
        let primary_keys = schema
            .iter()
            .filter(|column| {
                column
                    .get("pk")
                    .and_then(Value::as_i64)
                    .is_some_and(i64::is_positive)
            })
            .collect::<Vec<_>>();
        let pk = match primary_keys.as_slice() {
            [only] => only
                .get("name")
                .and_then(Value::as_str)
                .ok_or("primary key has no name")?,
            _ => {
                return Err(format!(
                    "table has no single-column primary key: {table_name}"
                ))
            }
        };
        let escaped_key = key.replace('\'', "''");
        // The key is the table's primary key, so at most one row answers.
        let rows = run_sql_rows(
            &conn,
            &format!(
                "SELECT * FROM {table_sql} WHERE {} = '{escaped_key}'",
                sqlite_identifier(pk)?
            ),
        )?;
        return Ok(
            json!({"path": path, "table": table_name, "schema": schema, "row": rows.into_iter().next()}),
        );
    }
    let mut clauses = Vec::new();
    if let Some(where_clause) = string_input(input, "where") {
        clauses.push(format!("WHERE {where_clause}"));
    }
    if let Some(order) = string_input(input, "order") {
        clauses.push(format!("ORDER BY {order}"));
    }
    let limit = count_input(input, "limit", "read_sqlite reading a table's rows")?;
    clauses.push(format!("LIMIT {limit}"));
    clauses.push(format!("OFFSET {offset}"));
    let rows = run_sql_rows(
        &conn,
        &format!("SELECT * FROM {table_sql} {}", clauses.join(" ")),
    )?;
    Ok(
        json!({"path": path, "table": table_name, "schema": schema, "rows": rows, "limit": limit, "offset": offset}),
    )
}
