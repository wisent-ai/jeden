use super::store::entities::{edge, memory};
use super::{MemoryEdge, MemoryRelation, MemorySource};
use crate::fleet::sql;
use sea_orm::sea_query::{Condition, OnConflict};
use sea_orm::{
    ActiveValue::Set, ColumnTrait, ConnectionTrait, EntityTrait, PaginatorTrait, QueryFilter,
    QueryOrder,
};
use std::collections::{HashMap, HashSet};

pub(super) async fn add_edge(
    db: &impl ConnectionTrait,
    from_id: &str,
    to_id: &str,
    relation: MemoryRelation,
    source: &MemorySource,
) -> Result<(), String> {
    if from_id == to_id {
        return Err("memory edge cannot be self-referential".into());
    }
    let exists = memory::Entity::find()
        .filter(memory::Column::Id.is_in([from_id.to_owned(), to_id.to_owned()]))
        .count(db)
        .await
        .map_err(sql)?;
    if exists != 2 {
        return Err("memory edge endpoint does not exist".into());
    }
    let provenance = serde_json::to_string(source).map_err(|e| e.to_string())?;
    let mut pairs = vec![(from_id, to_id)];
    if relation != MemoryRelation::Supports {
        pairs.push((to_id, from_id));
    }
    for (from, to) in pairs {
        edge::Entity::insert(edge::ActiveModel {
            from_id: Set(from.to_owned()),
            to_id: Set(to.to_owned()),
            relation: Set(relation.as_str().to_owned()),
            created_at: Set(super::now_ms()),
            provenance_json: Set(provenance.clone()),
        })
        .on_conflict(
            OnConflict::columns([
                edge::Column::FromId,
                edge::Column::ToId,
                edge::Column::Relation,
            ])
            .do_nothing()
            .to_owned(),
        )
        .exec_without_returning(db)
        .await
        .map_err(sql)?;
    }
    Ok(())
}

pub(super) async fn edges(
    db: &impl ConnectionTrait,
    memory_id: &str,
) -> Result<Vec<MemoryEdge>, String> {
    Ok(edge::Entity::find()
        .filter(
            Condition::any()
                .add(edge::Column::FromId.eq(memory_id.to_owned()))
                .add(edge::Column::ToId.eq(memory_id.to_owned())),
        )
        .order_by_asc(edge::Column::CreatedAt)
        .order_by_asc(edge::Column::FromId)
        .order_by_asc(edge::Column::ToId)
        .all(db)
        .await
        .map_err(sql)?
        .into_iter()
        .map(|row| MemoryEdge {
            relation: MemoryRelation::parse(&row.relation).unwrap_or(MemoryRelation::Supports),
            provenance: serde_json::from_str(&row.provenance_json).unwrap_or(MemorySource {
                origin: "unknown".into(),
                session_id: None,
                entry_id: None,
            }),
            from_id: row.from_id,
            to_id: row.to_id,
            created_at: row.created_at,
        })
        .collect())
}

pub(super) async fn conflict_groups(
    db: &impl ConnectionTrait,
    ids: &[String],
) -> Result<HashMap<String, String>, String> {
    let wanted = ids.iter().cloned().collect::<HashSet<_>>();
    let mut graph: HashMap<String, Vec<String>> = HashMap::new();
    for row in edge::Entity::find()
        .filter(edge::Column::Relation.eq("conflicts"))
        .filter(edge::Column::FromId.is_in(ids.to_vec()))
        .all(db)
        .await
        .map_err(sql)?
    {
        if wanted.contains(&row.from_id) && wanted.contains(&row.to_id) {
            graph.entry(row.from_id).or_default().push(row.to_id);
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
