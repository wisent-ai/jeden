use super::{MemoryEdge, MemoryRelation, MemorySource};
use rusqlite::{params, Connection};
use std::collections::{HashMap, HashSet};

pub(super) fn add_edge(
    conn: &Connection,
    from_id: &str,
    to_id: &str,
    relation: MemoryRelation,
    source: &MemorySource,
) -> Result<(), String> {
    if from_id == to_id {
        return Err("memory edge cannot be self-referential".into());
    }
    let exists: i64 = conn
        .query_row(
            "SELECT count(*) FROM memories WHERE id IN (?1,?2)",
            params![from_id, to_id],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    if exists != 2 {
        return Err("memory edge endpoint does not exist".into());
    }
    conn.execute(
        "INSERT OR IGNORE INTO memory_edges(from_id,to_id,relation,created_at,provenance_json) VALUES(?1,?2,?3,?4,?5)",
        params![from_id, to_id, relation.as_str(), super::now_ms(), serde_json::to_string(source).map_err(|e| e.to_string())?],
    ).map_err(|e| e.to_string())?;
    if relation != MemoryRelation::Supports {
        conn.execute(
            "INSERT OR IGNORE INTO memory_edges(from_id,to_id,relation,created_at,provenance_json) VALUES(?1,?2,?3,?4,?5)",
            params![to_id, from_id, relation.as_str(), super::now_ms(), serde_json::to_string(source).map_err(|e| e.to_string())?],
        ).map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub(super) fn edges(conn: &Connection, memory_id: &str) -> Result<Vec<MemoryEdge>, String> {
    let mut stmt = conn.prepare(
        "SELECT from_id,to_id,relation,created_at,provenance_json FROM memory_edges WHERE from_id=?1 OR to_id=?1 ORDER BY created_at,from_id,to_id"
    ).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([memory_id], |row| {
            let relation: String = row.get(2)?;
            let provenance: String = row.get(4)?;
            Ok(MemoryEdge {
                from_id: row.get(0)?,
                to_id: row.get(1)?,
                relation: MemoryRelation::parse(&relation).unwrap_or(MemoryRelation::Supports),
                created_at: row.get(3)?,
                provenance: serde_json::from_str(&provenance).unwrap_or(MemorySource {
                    origin: "unknown".into(),
                    session_id: None,
                    entry_id: None,
                }),
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    Ok(rows)
}

pub(super) fn conflict_groups(
    conn: &Connection,
    ids: &[String],
) -> Result<HashMap<String, String>, String> {
    let wanted = ids.iter().cloned().collect::<HashSet<_>>();
    let mut graph: HashMap<String, Vec<String>> = HashMap::new();
    let mut stmt = conn
        .prepare("SELECT from_id,to_id FROM memory_edges WHERE relation='conflicts'")
        .map_err(|e| e.to_string())?;
    for row in stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|e| e.to_string())?
    {
        let (a, b) = row.map_err(|e| e.to_string())?;
        if wanted.contains(&a) && wanted.contains(&b) {
            graph.entry(a).or_default().push(b);
        }
    }
    let mut groups = HashMap::new();
    let mut visited = HashSet::new();
    for id in ids {
        if visited.contains(id) || !graph.contains_key(id) {
            continue;
        }
        let mut stack = vec![id.clone()];
        let mut members = Vec::new();
        while let Some(node) = stack.pop() {
            if !visited.insert(node.clone()) {
                continue;
            }
            members.push(node.clone());
            stack.extend(graph.get(&node).into_iter().flatten().cloned());
        }
        members.sort();
        let group = format!("conflict:{}", members.join(":"));
        for member in members {
            groups.insert(member, group.clone());
        }
    }
    Ok(groups)
}
