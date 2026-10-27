//! Jeden's own Brama: the standalone gateway this process starts for itself
//! when the shared Brama does not serve it and Jeden holds subscription
//! grants of its own. The sign-in that banks a grant is
//! [`sign_in`](self::sign_in).
//!
//! The grants live in `~/.jeden/brama/grants.json`, an owner-only file Brama's
//! standalone mode reads when it starts and writes back after every donation,
//! renewal and removal (`BRAMA_STANDALONE_GRANTS_FILE`), so a renewal that
//! rotates a refresh token is kept. The gateway is `brama serve --port 0
//! --local-credentials-stdin`, the mode a desktop client runs privately: no
//! Skarbiec, no shared Brama, an empty provider map on its standard input,
//! one client identity with a bearer this process makes at random, its state
//! under `~/.jeden/brama/state`, its subscription overlay in
//! `~/.jeden/brama/donated-subscriptions.json`. The listener's address is
//! read from the line Brama writes once it is bound, never from a clock, and
//! the gateway is ended with this process (`stop`). Provider code is Brama's;
//! nothing here speaks to a provider.
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

mod ownership;
use ownership::{lease, private_directory};
mod sign_in;
mod process;
use process::{ended, held, live, GatewayProcess};

pub(super) use process::running;
pub(crate) use process::stop;

pub(crate) use sign_in::{sign_in, sign_in_begin, sign_in_complete};

/// The line Brama writes once its listener is bound (its
/// `core/server/lifecycle`), followed by the bound address.
const LISTENING: &str = "brama server listening on ";

/// The client identity this process presents to its own gateway.
const CLIENT: &str = "jeden";

/// The program the private gateway is.
const PROGRAM: &str = "brama";


/// Where the private gateway keeps its grants, state and overlay.
fn directory() -> PathBuf {
    crate::dirs_home().join(".jeden").join("brama")
}

/// The owner-only grants file the private gateway reads and writes.
pub(crate) fn grants_file() -> PathBuf {
    directory().join("grants.json")
}

/// Whether Jeden holds subscription grants for its own gateway: the grants
/// file is a map that names at least one. A file that cannot be read, or is
/// no map, refuses naming it, so a grants file every subscription left cannot
/// read as a grant.
pub(crate) fn holds_grants() -> Result<bool, String> {
    let path = grants_file();
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(format!(
                "Jeden's grants file {} cannot be read: {error}",
                path.display()
            ))
        }
    };
    let grants: serde_json::Value = serde_json::from_str(&text).map_err(|error| {
        format!(
            "Jeden's grants file {} is not a map from subscription to grant: {error}",
            path.display()
        )
    })?;
    let map = grants.as_object().ok_or_else(|| {
        format!(
            "Jeden's grants file {} is not a map from subscription to grant",
            path.display()
        )
    })?;
    Ok(!map.is_empty())
}

/// What stands in when the shared Brama does not serve Jeden, as status
/// shows it: its own gateway on the subscriptions signed into it, none, or
/// why the grants file cannot be examined.
pub(crate) fn described() -> String {
    if let Some(refusal) = process::refusal() {
        return refusal;
    }
    match holds_grants() {
        Ok(true) => format!("subscriptions signed in, kept in {}", grants_file().display()),
        Ok(false) => "no subscription signed in; jeden token sign-in PROVIDER --account EMAIL --reason TEXT signs one in".into(),
        Err(cause) => format!("refused: {cause}"),
    }
}

/// A started private gateway: the address to send model work to and the
/// bearer it accepts from this process.
#[derive(Clone)]
pub(crate) struct Gateway {
    pub(crate) endpoint: String,
    pub(crate) bearer: String,
}


/// The gateway this process runs, started now if it runs none: `brama
/// serve` in standalone mode on Jeden's grants, answered once its listener is
/// bound, or why it did not bind, with what it said. One lock is held
/// across the check, the spawn and the store, so calls arriving together
/// start one gateway, not one each.
pub(crate) fn start() -> Result<Gateway, String> {
    let mut running = held();
    if let Some(gateway) = live(&mut running)? {
        return Ok(gateway);
    }
    let dir = directory();
    let leased = lease()?;
    private_directory(&dir.join("state"))?;
    let bearer = uuid::Uuid::new_v4().simple().to_string();
    let identities = serde_json::json!([{ "client_id": CLIENT, "token": bearer }]).to_string();
    let mut child = Command::new(PROGRAM)
        .args(["serve", "--port", "0", "--local-credentials-stdin"])
        .env_remove("BRAMA_BIND_ADDRESS")
        .env("BRAMA_MODEL_ROUTER_CLIENT_IDENTITIES", identities)
        .env("BRAMA_STATE_DIR", dir.join("state"))
        .env("BRAMA_STANDALONE_GRANTS_FILE", grants_file())
        .env(
            "BRAMA_DONATED_SUBSCRIPTIONS_FILE",
            dir.join("donated-subscriptions.json"),
        )
        // Jeden is its own gateway's console: it signs its subscriptions in
        // there and reads the catalogue those subscriptions pay for.
        .env("BRAMA_STANDALONE_CONSOLE_CLIENT", CLIENT)
        .env("NO_COLOR", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("brama serve could not start for Jeden's own gateway: {error}"))?;
    // An empty provider map: the gateway serves only the subscriptions whose
    // grants the grants file holds.
    let handed = match child.stdin.take() {
        Some(mut input) => input.write_all(b"{}").map_err(|error| error.to_string()),
        None => Err("its standard input was not opened".to_owned()),
    };
    if let Err(cause) = handed {
        return Err(ended(
            child,
            leased,
            format!("brama serve was not handed its provider map: {cause}"),
            &[],
            &mut running,
        ));
    }
    let Some(stderr) = child.stderr.take() else {
        return Err(ended(
            child,
            leased,
            "brama serve's standard error was not opened".to_owned(),
            &[],
            &mut running,
        ));
    };
    let mut lines = BufReader::new(stderr).lines();
    let mut said = Vec::new();
    let address = loop {
        match lines.next() {
            Some(Ok(line)) => match line.split_once(LISTENING) {
                Some((_, rest)) => match rest.split_whitespace().next() {
                    Some(address) => break address.to_owned(),
                    None => said.push(line),
                },
                None => said.push(line),
            },
            Some(Err(error)) => {
                return Err(ended(
                    child,
                    leased,
                    format!("brama serve's standard error cannot be read: {error}"),
                    &said,
                    &mut running,
                ))
            }
            None => {
                return Err(ended(
                    child,
                    leased,
                    "brama serve ended before its listener was bound".to_owned(),
                    &said,
                    &mut running,
                ))
            }
        }
    };
    // The gateway keeps logging; its pipe is drained so it never blocks on it.
    std::thread::spawn(move || lines.for_each(drop));
    let gateway = Gateway {
        endpoint: format!("http://{address}"),
        bearer,
    };
    *running = Some(GatewayProcess::serving(leased, child, gateway.clone()));
    Ok(gateway)
}

