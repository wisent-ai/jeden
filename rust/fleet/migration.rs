//! The schema of the fleet database `jeden`, as `sea-orm-migration`
//! migrations recorded in `seaql_migrations`. Each owner declares its own
//! tables; a later change is a new migration here, never an edit of one
//! that already ran.

use sea_orm_migration::prelude::*;

pub(super) struct Migrator;

impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(Tables {
                name: "m20260928_000001_pursuit",
                schema: crate::autonomy::requests::SCHEMA,
            }),
            Box::new(Tables {
                name: "m20260928_000002_collab_relay",
                schema: crate::collab::RELAY_SCHEMA,
            }),
            Box::new(Tables {
                name: "m20260928_000003_memory",
                schema: crate::memory::SCHEMA,
            }),
        ]
    }
}

/// One owner's tables, created as one migration.
struct Tables {
    name: &'static str,
    schema: &'static str,
}

impl MigrationName for Tables {
    fn name(&self) -> &str {
        self.name
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Tables {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(self.schema)
            .await
            .map(|_| ())
    }
}
