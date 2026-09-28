use super::{MemoryEdge, MemoryRelation, MemorySource};
use crate::fleet::sql;
use postgres::GenericClient;
use std::collections::{HashMap, HashSet};

pub(super) fn add_edge(
    client: &mut impl GenericClient,
    from_id: &str,
    to_id: &str,
    relation: MemoryRelation,
    source: &MemorySource,
) -> Result<(), String> {
    if from_id == to_id {
        return Err("memory edge cannot be self-referential".into());
    }
    let exists: i64 = client
        .query_one(
            "SELECT count(*) FROM memories WHERE id IN ($1,$2)",
            &[&from_id, &to_id],
        )
        .map_err(sql)?
        .get(0);
    if exists != 2 {
        return Err("memory edge endpoint does not exist".into());
    }
    let provenance = serde_json::to_string(source).map_err(|e| e.to_string())?;
    let mut pairs = vec![(from_id, to_id)];
    if relation != MemoryRelation::Supports {
        pairs.push((to_id, from_id));
    }
    for (from, to) in pairs {
        client
            .execute(
                "INSERT INTO memory_edges(from_id,to_id,relation,created_at,provenance_json)
                 VALUES($1,$2,$3,$4,$5) ON CONFLICT DO NOTHING",
                &[
                    &from,
                    &to,
                    &relation.as_str(),
                    &super::now_ms(),
                    &provenance,
                ],
            )
            .map_err(sql)?;
    }
    Ok(())
}

pub(super) fn edges(
    client: &mut impl GenericClient,
    memory_id: &str,
) -> Result<Vec<MemoryEdge>, String> {
    Ok(client
        .query(
            "SELECT from_id,to_id,relation,created_at,provenance_json FROM memory_edges
             WHERE from_id=$1 OR to_id=$1 ORDER BY created_at,from_id,to_id",
            &[&memory_id],
        )
        .map_err(sql)?
        .into_iter()
        .map(|row| {
            let relation: String = row.get(2);
            let provenance: String = row.get(4);
            MemoryEdge {
                from_id: row.get(0),
                to_id: row.get(1),
                relation: MemoryRelation::parse(&relation).unwrap_or(MemoryRelation::Supports),
                created_at: row.get(3),
                provenance: serde_json::from_str(&provenance).unwrap_or(MemorySource {
                    origin: "unknown".into(),
                    session_id: None,
                    entry_id: None,
                }),
            }
        })
        .collect())
}

pub(super) fn conflict_groups(
    client: &mut impl GenericClient,
    ids: &[String],
) -> Result<HashMap<String, String>, String> {
    let wanted = ids.iter().cloned().collect::<HashSet<_>>();
    let mut graph: HashMap<String, Vec<String>> = HashMap::new();
    for row in client
        .query(
            "SELECT from_id,to_id FROM memory_edges WHERE relation='conflicts' AND from_id=ANY($1)",
            &[&ids],
        )
        .map_err(sql)?
    {
        let (a, b): (String, String) = (row.get(0), row.get(1));
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
