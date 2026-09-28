//! Where Jeden keeps durable shared state: the fleet database `jeden`,
//! reached the one way every Wisent product reaches its database —
//! `stado_database` (Stado resolve, the Skarbiec route, the
//! `jeden-database-client` bearer) — as a SeaORM connection. Tables are
//! SeaORM entities in the modules that own them; `migration` creates them.
//!
//! Much of Jeden calls storage from synchronous code, so it uses Stado's
//! shared synchronous client and hands it entity work with `run`; the
//! process opens that client once, on first use.

mod migration;

use sea_orm::DatabaseConnection;
use sea_orm_migration::MigratorTrait;
use stado_database::sync::Client;
use std::future::Future;
use std::sync::{Arc, Mutex};

/// `HOME` holding `.stado/` when the process's own `HOME` is isolated, as in
/// the pursuit journeys; Jeden then still reaches the fleet as this host.
const STADO_HOME_VARIABLE: &str = "JEDEN_STADO_HOME";

static CLIENT: Mutex<Option<Arc<Client>>> = Mutex::new(None);

fn failed(step: &str, detail: impl std::fmt::Display) -> String {
    format!("jeden fleet database: {step}: {detail}")
}

fn open() -> Result<Client, String> {
    let database = stado_database::FleetDatabase::for_product("jeden", STADO_HOME_VARIABLE)
        .map_err(|error| error.to_string())?;
    let client = Client::connect(&database).map_err(|error| error.to_string())?;
    client
        .run(|db| async move { migration::Migrator::up(&db, None).await })
        .map_err(|error| failed("creating Jeden's tables", error))?
        .map_err(|error| failed("creating Jeden's tables", error))?;
    Ok(client)
}

/// The process's client, opened on first use.
fn client() -> Result<Arc<Client>, String> {
    let mut slot = CLIENT
        .lock()
        .map_err(|_| failed("reading the connection", "its lock failed"))?;
    if let Some(client) = slot.as_ref() {
        return Ok(Arc::clone(client));
    }
    let client = Arc::new(open()?);
    *slot = Some(Arc::clone(&client));
    Ok(client)
}

/// Run `work` against the shared connection and wait for its answer.
pub(crate) fn run_db<R, F>(work: impl FnOnce(DatabaseConnection) -> F) -> Result<R, String>
where
    R: Send + 'static,
    F: Future<Output = Result<R, String>> + Send + 'static,
{
    client()?
        .run(work)
        .map_err(|error| failed("running an operation", error))?
}

/// A database error as the text Jeden's surfaces show.
pub(crate) fn sql(error: sea_orm::DbErr) -> String {
    failed("query", error)
}
