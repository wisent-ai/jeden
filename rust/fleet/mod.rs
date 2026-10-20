//! Jeden's durable shared state. The database is an explicitly configured
//! PostgreSQL, MySQL or SQLite URL, not an implicit Skarbiec credential; with
//! none configured, Jeden's own SQLite file under `~/.jeden` stands in and the
//! process says so on standard error. Stado's synchronous SeaORM client still
//! supplies the connection and migrations; its direct environment route does
//! not contact Stado or Skarbiec. Tables are SeaORM entities in their owning
//! modules.

mod cancellation;
pub(crate) use cancellation::with_cancellation;

mod migration;

use sea_orm::DatabaseConnection;
use sea_orm_migration::MigratorTrait;
use stado_database::sync::Client;
use std::future::Future;
use std::sync::{Arc, Condvar, Mutex};

/// Home used by the database client for its descriptor; a direct URL does
/// not resolve any service or credential there.
const STADO_HOME_VARIABLE: &str = "JEDEN_STADO_HOME";

enum ConnectionState {
    Closed,
    Opening,
    Ready(Arc<Client>),
}

static CLIENT: Mutex<ConnectionState> = Mutex::new(ConnectionState::Closed);
static CONNECTION_CHANGED: Condvar = Condvar::new();

/// A failed or panicking initializer must not leave its waiters stranded.
struct Opening;

impl Drop for Opening {
    fn drop(&mut self) {
        if let Ok(mut slot) = CLIENT.lock() {
            if matches!(*slot, ConnectionState::Opening) {
                *slot = ConnectionState::Closed;
            }
        }
        CONNECTION_CHANGED.notify_all();
    }
}

fn failed(step: &str, detail: impl std::fmt::Display) -> String {
    format!("jeden fleet database: {step}: {detail}")
}

/// The variable naming Jeden's database, and the file that stands in for it.
const DATABASE_VARIABLE: &str = "JEDEN_DATABASE_URL";
const STAND_IN_FILE: &str = ".jeden/state.sqlite3";

/// With no database configured, Jeden's own SQLite file serves: named in
/// `JEDEN_DATABASE_URL` for this process, created on first use, and said on
/// standard error. A configured database that does not answer is refused
/// instead: moving state to another store silently would split it in two.
fn stand_in() -> Result<(), String> {
    let path = crate::dirs_home().join(STAND_IN_FILE);
    let directory = path
        .parent()
        .expect("the stand-in file sits in a directory");
    std::fs::create_dir_all(directory).map_err(|error| {
        failed(
            "configure storage",
            format!("create {}: {error}", directory.display()),
        )
    })?;
    let url = format!("sqlite://{}?mode=rwc", path.display());
    unsafe { std::env::set_var(DATABASE_VARIABLE, &url) };
    eprintln!(
        "jeden: no fleet database is configured ({DATABASE_VARIABLE}); Jeden's durable state is the local SQLite file {}",
        path.display()
    );
    Ok(())
}

fn open() -> Result<Client, String> {
    match std::env::var(DATABASE_VARIABLE) {
        Ok(value) if !value.trim().is_empty() => {}
        Ok(_) | Err(std::env::VarError::NotPresent) => stand_in()?,
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err(failed(
                "configure storage",
                "JEDEN_DATABASE_URL cannot be read: value is not Unicode",
            ))
        }
    }
    let database = stado_database::FleetDatabase::for_product("jeden", STADO_HOME_VARIABLE)
        .map_err(|error| error.to_string())?;
    let cancellation = cancellation::database();
    let client = match cancellation.as_ref() {
        Some(cancellation) => Client::connect_cancellable(&database, cancellation),
        None => Client::connect(&database),
    }
    .map_err(|error| error.to_string())?;
    run_on_client(&client, |db| async move {
        migration::Migrator::up(&db, None).await
    })
    .map_err(|error| failed("creating Jeden's tables", error))?
    .map_err(|error| failed("creating Jeden's tables", error))?;
    Ok(client)
}

/// The process's client, opened on first use.
fn client() -> Result<Arc<Client>, String> {
    cancellation::check()?;
    {
        let slot = CLIENT
            .lock()
            .map_err(|_| failed("reading the connection", "its lock failed"))?;
        if let ConnectionState::Ready(client) = &*slot {
            return Ok(Arc::clone(client));
        }
    }
    // Register before taking CLIENT: cancellation callbacks take that lock
    // to synchronize notification with the condvar's atomic unlock-and-wait.
    let _wake = cancellation::wake_when_cancelled(|| {
        let _slot = CLIENT.lock();
        CONNECTION_CHANGED.notify_all();
    });
    loop {
        cancellation::check()?;
        let mut slot = CLIENT
            .lock()
            .map_err(|_| failed("reading the connection", "its lock failed"))?;
        match &*slot {
            ConnectionState::Ready(client) => return Ok(Arc::clone(client)),
            ConnectionState::Opening => {
                cancellation::check()?;
                drop(CONNECTION_CHANGED.wait(slot).map_err(|_| {
                    failed("waiting for connection initialization", "its lock failed")
                })?);
            }
            ConnectionState::Closed => {
                *slot = ConnectionState::Opening;
                drop(slot);
                let _opening = Opening;
                let client = Arc::new(open()?);
                let mut slot = CLIENT
                    .lock()
                    .map_err(|_| failed("publishing the connection", "its lock failed"))?;
                *slot = ConnectionState::Ready(Arc::clone(&client));
                drop(slot);
                return Ok(client);
            }
        }
    }
}

/// Run `work` against the shared connection and wait for its answer.
pub(crate) fn run_db<R, F>(work: impl FnOnce(DatabaseConnection) -> F) -> Result<R, String>
where
    R: Send + 'static,
    F: Future<Output = Result<R, String>> + Send + 'static,
{
    let client = client()?;
    cancellation::check()?;
    run_on_client(&client, work).map_err(|error| failed("running an operation", error))?
}

fn run_on_client<T, F>(
    client: &Client,
    work: impl FnOnce(DatabaseConnection) -> F,
) -> stado_database::sync::Result<T>
where
    T: Send + 'static,
    F: Future<Output = T> + Send + 'static,
{
    match cancellation::database() {
        Some(cancellation) => client.run_cancellable(&cancellation, work),
        None => client.run(work),
    }
}

/// A database error as the text Jeden's surfaces show.
pub(crate) fn sql(error: sea_orm::DbErr) -> String {
    failed("query", error)
}
