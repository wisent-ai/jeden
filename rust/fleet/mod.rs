//! Where Jeden keeps durable shared state: the fleet database `jeden`,
//! reached the one way every Wisent product reaches its database —
//! `stado_database::connect` (Stado resolve, the Skarbiec route, the
//! `jeden-database-client` bearer) — as a SeaORM connection. Tables are
//! SeaORM entities in the modules that own them; `migration` creates them.
//!
//! Much of Jeden calls storage from synchronous code, some of it inside a
//! Tokio worker where blocking on a future is not allowed. One dedicated
//! thread owns a runtime and the connection; every operation is sent to it
//! and answered back. The process shares that one connection pool.

mod migration;

use sea_orm::DatabaseConnection;
use sea_orm_migration::MigratorTrait;
use std::future::Future;
use std::pin::Pin;
use std::sync::{mpsc, Mutex};

/// `HOME` holding `.stado/` when the process's own `HOME` is isolated, as in
/// the pursuit journeys; Jeden then still reaches the fleet as this host.
const STADO_HOME_VARIABLE: &str = "JEDEN_STADO_HOME";

type Answer = Pin<Box<dyn Future<Output = ()> + Send>>;
/// One operation for the database thread.
type Job = Box<dyn FnOnce(DatabaseConnection) -> Answer + Send>;

static JOBS: Mutex<Option<mpsc::Sender<Job>>> = Mutex::new(None);

fn failed(step: &str, detail: impl std::fmt::Display) -> String {
    format!("jeden fleet database: {step}: {detail}")
}

async fn open() -> Result<DatabaseConnection, String> {
    let database = stado_database::FleetDatabase::for_product("jeden", STADO_HOME_VARIABLE)
        .map_err(|error| error.to_string())?;
    let connection = stado_database::connect(&database)
        .await
        .map_err(|error| error.to_string())?;
    migration::Migrator::up(&connection, None)
        .await
        .map_err(|error| failed("creating Jeden's tables", error))?;
    Ok(connection)
}

fn start() -> Result<mpsc::Sender<Job>, String> {
    let (jobs, inbox) = mpsc::channel::<Job>();
    let (ready, connected) = mpsc::sync_channel::<Result<(), String>>(1);
    std::thread::Builder::new()
        .name("jeden-fleet-database".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    let _ = ready.send(Err(failed("starting the database runtime", error)));
                    return;
                }
            };
            let connection = match runtime.block_on(open()) {
                Ok(connection) => {
                    let _ = ready.send(Ok(()));
                    connection
                }
                Err(error) => {
                    let _ = ready.send(Err(error));
                    return;
                }
            };
            // The pool replaces a dropped connection by itself.
            for job in inbox {
                runtime.block_on(job(connection.clone()));
            }
        })
        .map_err(|error| failed("starting the database thread", error))?;
    connected
        .recv()
        .map_err(|_| failed("starting the database thread", "it ended before connecting"))??;
    Ok(jobs)
}

/// Run `work` against the shared connection and wait for its answer.
pub(crate) fn run_db<R, F>(
    work: impl FnOnce(DatabaseConnection) -> F + Send + 'static,
) -> Result<R, String>
where
    R: Send + 'static,
    F: Future<Output = Result<R, String>> + Send + 'static,
{
    let jobs = {
        let mut slot = JOBS
            .lock()
            .map_err(|_| failed("reading the connection", "its lock failed"))?;
        if slot.is_none() {
            *slot = Some(start()?);
        }
        slot.clone()
            .ok_or_else(|| failed("reading the connection", "it was not opened"))?
    };
    let (reply, answer) = mpsc::sync_channel::<Result<R, String>>(1);
    let job: Job = Box::new(move |connection| {
        Box::pin(async move {
            let _ = reply.send(work(connection).await);
        })
    });
    if jobs.send(job).is_err() {
        if let Ok(mut slot) = JOBS.lock() {
            *slot = None;
        }
        return Err(failed(
            "sending an operation",
            "the database thread has ended",
        ));
    }
    answer.recv().map_err(|_| {
        failed(
            "reading an answer",
            "the database thread ended mid-operation",
        )
    })?
}

/// A database error as the text Jeden's surfaces show.
pub(crate) fn sql(error: sea_orm::DbErr) -> String {
    failed("query", error)
}
