//! Where pursuit requests keep their state: the fleet database `jeden`.
//! Stado names the Skarbiec item that holds its address (`stado database
//! resolve`), and Skarbiec answers the pooler URL and the provider's root
//! certificate to the consumer `jeden-database-client`, whose bearer Stado
//! keeps in `~/.stado/jeden-database-client-skarbiec-token`.
//!
//! The synchronous Postgres client runs its own runtime, which may not be
//! entered from a Tokio worker; one dedicated thread owns the connection and
//! every operation is sent to it and answered back. The process shares one
//! connection, opened on first use.

use postgres::config::SslMode;
use postgres::Client;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::Value;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{mpsc, Mutex};

const DATABASE: &str = "jeden";
/// Who asks Stado's directory; the database lists `jeden` as consumer.
const DIRECTORY_CONSUMER: &str = "jeden";
/// Who reads the credential item; it may read exactly the two fields below.
const CREDENTIAL_CONSUMER: &str = "jeden-database-client";
const TOKEN_FILE: &str = "jeden-database-client-skarbiec-token";

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS pursuit_values (request TEXT NOT NULL, key TEXT NOT NULL,
        data TEXT NOT NULL, PRIMARY KEY(request,key));
    CREATE TABLE IF NOT EXISTS pursuit_stages (request TEXT NOT NULL, position BIGINT NOT NULL,
        data TEXT NOT NULL, PRIMARY KEY(request,position));
    CREATE TABLE IF NOT EXISTS pursuit_calls (request TEXT NOT NULL, id TEXT NOT NULL,
        model TEXT NOT NULL, catalog_revision TEXT NOT NULL, reserved TEXT NOT NULL, actual TEXT,
        PRIMARY KEY(request,id));";

#[derive(Deserialize)]
struct Resolution {
    credential_item: String,
}

#[derive(Deserialize)]
struct Route {
    url: String,
}

fn failed(step: &str, detail: impl std::fmt::Display) -> String {
    format!("jeden fleet database: {step}: {detail}")
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

fn stado() -> Result<PathBuf, String> {
    let stado = home().join(".stado/bin/stado");
    if stado.is_file() {
        Ok(stado)
    } else {
        Err(failed(
            "locating Stado",
            format!(
                "Stado is not installed at {}; pursuit state is found through Stado",
                stado.display()
            ),
        ))
    }
}

/// `stado <arguments>`, with exactly `environment` when one is given, so no
/// ambient variable selects the identity a credential read runs under.
fn run(arguments: &[&str], environment: Option<&[(&str, String)]>) -> Result<String, String> {
    let operation = format!("stado {}", arguments.join(" "));
    let mut command = Command::new(stado()?);
    command.args(arguments).stdin(Stdio::null());
    if let Some(environment) = environment {
        command.env_clear();
        for (name, value) in environment {
            command.env(name, value);
        }
    }
    let output = command
        .output()
        .map_err(|error| failed(&operation, format!("could not start: {error}")))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        return Err(failed(
            &operation,
            format!("exited {}: {}", output.status, detail.trim()),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn answer<T: DeserializeOwned>(arguments: &[&str]) -> Result<T, String> {
    let output = run(arguments, None)?;
    serde_json::from_str(&output).map_err(|error| {
        failed(
            &format!("stado {}", arguments.join(" ")),
            format!("unreadable JSON: {error}"),
        )
    })
}

/// A value answered as a JSON string, as `{"value": …}`, or as text.
fn decoded(output: &str) -> Option<String> {
    let value = match serde_json::from_str::<Value>(output) {
        Ok(Value::String(value)) => value,
        Ok(Value::Object(object)) => object.get("value")?.as_str()?.to_owned(),
        Ok(_) => return None,
        Err(_) => output.to_owned(),
    };
    let value = value.trim().to_owned();
    (!value.is_empty()).then_some(value)
}

fn read_field(route: &str, item: &str, field: &str) -> Result<String, String> {
    let environment = [
        ("HOME", home().display().to_string()),
        (
            "PATH",
            std::env::var("PATH").unwrap_or_else(|_| "/usr/local/bin:/usr/bin:/bin".into()),
        ),
        ("TMPDIR", std::env::temp_dir().display().to_string()),
        ("STADO_CREDENTIALS_ADMIN_URL", route.to_owned()),
        (
            "STADO_CREDENTIALS_ADMIN_CONSUMER",
            CREDENTIAL_CONSUMER.to_owned(),
        ),
        (
            "STADO_CREDENTIALS_ADMIN_TOKEN_FILE",
            home().join(".stado").join(TOKEN_FILE).display().to_string(),
        ),
    ];
    let output = run(
        &["secrets", "get", item, "--field", field],
        Some(&environment),
    )?;
    decoded(&output).ok_or_else(|| {
        failed(
            &format!("stado secrets get {item} --field {field} as {CREDENTIAL_CONSUMER}"),
            "answered an empty value",
        )
    })
}

fn connect() -> Result<Client, String> {
    let resolution: Resolution = answer(&[
        "database",
        "resolve",
        DATABASE,
        "--consumer",
        DIRECTORY_CONSUMER,
        "--json",
    ])?;
    let route: Route = answer(&[
        "service",
        "directory",
        "connect",
        "skarbiec",
        "--consumer",
        DIRECTORY_CONSUMER,
        "--json",
    ])?;
    let item = resolution.credential_item;
    let url = read_field(&route.url, &item, "pooler_url")?;
    let certificate = read_field(&route.url, &item, "ca_certificate")?;
    let certificate =
        native_tls::Certificate::from_pem(certificate.as_bytes()).map_err(|error| {
            failed(
                &format!("{item}#ca_certificate"),
                format!("not a PEM certificate: {error}"),
            )
        })?;
    let tls = native_tls::TlsConnector::builder()
        .add_root_certificate(certificate)
        .build()
        .map_err(|error| failed("building the TLS connector", error))?;
    let mut config: postgres::Config = url.parse().map_err(|error| {
        failed(
            &format!("{item}#pooler_url"),
            format!("not a Postgres connection URL: {error}"),
        )
    })?;
    config.ssl_mode(SslMode::Require);
    let mut client = config
        .connect(postgres_native_tls::MakeTlsConnector::new(tls))
        .map_err(|error| {
            failed(
                &format!("connecting to {DATABASE} through {item}#pooler_url"),
                error,
            )
        })?;
    client
        .batch_execute(SCHEMA)
        .map_err(|error| failed("creating the pursuit tables", error))?;
    Ok(client)
}

type Job = Box<dyn FnOnce(&mut Client) + Send>;

static JOBS: Mutex<Option<mpsc::Sender<Job>>> = Mutex::new(None);

fn start() -> Result<mpsc::Sender<Job>, String> {
    let (jobs, inbox) = mpsc::channel::<Job>();
    let (ready, connected) = mpsc::sync_channel::<Result<(), String>>(1);
    std::thread::Builder::new()
        .name("jeden-fleet-database".into())
        .spawn(move || {
            let mut client = match connect() {
                Ok(client) => {
                    let _ = ready.send(Ok(()));
                    client
                }
                Err(error) => {
                    let _ = ready.send(Err(error));
                    return;
                }
            };
            for job in inbox {
                job(&mut client);
            }
        })
        .map_err(|error| failed("starting the database thread", error))?;
    connected
        .recv()
        .map_err(|_| failed("starting the database thread", "it ended before connecting"))??;
    Ok(jobs)
}

/// Run `work` against the shared connection and wait for its answer.
pub(super) fn run_db<R: Send + 'static>(
    work: impl FnOnce(&mut Client) -> Result<R, String> + Send + 'static,
) -> Result<R, String> {
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
    if jobs
        .send(Box::new(move |client| {
            let _ = reply.send(work(client));
        }))
        .is_err()
    {
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

/// A Postgres error as the text the pursuit surfaces show.
pub(super) fn sql(error: postgres::Error) -> String {
    failed("query", error)
}
