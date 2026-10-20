//! The account a Jeden session runs on: a lease from Brama's subscription pool.
//!
//! Brama owns the pool and the register (`/v1/subscription-pool/leases`).
//! A session whose route names a provider Brama states a sessions limit for
//! takes a lease on that provider before its first call, signed as Jeden's
//! agent, and every request of the session names the leased member in
//! `x-brama-subscription` (and `x-brama-account` when the member declares
//! one). Brama answers a session that already holds a live lease with that
//! lease, so a turn, a resumed session or a restarted process lands on the
//! account it already runs on. `session` keeps what this process took and
//! moves a session to another account at its limit.

use super::*;
use serde::Deserialize;
use std::collections::BTreeMap;

mod session;

pub(crate) use session::{for_route, release_held, switch};

const LEASES: &str = "/v1/subscription-pool/leases";
/// The pool document, whose `accounts.capacity` states what each account has left.
const POOL: &str = "/v1/subscription-pool";
/// Who takes the lease, as Brama records it.
pub(crate) const HOLDER: &str = "jeden";
/// The request header naming the leased member, as Brama reads it.
const SUBSCRIPTION_HEADER: &str = "x-brama-subscription";
/// The request header naming the leased member's account.
const ACCOUNT_HEADER: &str = "x-brama-account";

/// One lease as Brama answers it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionLease {
    pub id: String,
    pub provider: String,
    pub subscription_id: String,
    #[serde(default)]
    pub account: Option<String>,
    pub session_id: String,
    pub holder: String,
    pub taken_at_ms: i64,
}

impl SessionLease {
    /// The headers every request of the session sends on this lease.
    pub(crate) fn headers(&self) -> Vec<(&'static str, String)> {
        let mut headers = vec![(
            SUBSCRIPTION_HEADER,
            format!("{}:{}", self.provider, self.subscription_id),
        )];
        if let Some(account) = self
            .account
            .as_deref()
            .filter(|account| !account.is_empty())
        {
            headers.push((ACCOUNT_HEADER, account.to_string()));
        }
        headers
    }

    /// `<subscription> (<account>)`, as the CLI and the session log name it.
    pub(crate) fn member(&self) -> String {
        match self
            .account
            .as_deref()
            .filter(|account| !account.is_empty())
        {
            Some(account) => format!("{} ({account})", self.subscription_id),
            None => self.subscription_id.clone(),
        }
    }
}

/// A lease taken. Brama's answer also counts the sessions the account
/// carries and any acquisition it started; `jeden lease list` reads those
/// from the register.
#[derive(Debug, Clone, Deserialize)]
pub struct Taken {
    pub lease: SessionLease,
}

/// The variable a program that starts a Jeden session and holds its lease
/// (Oko) hands that lease in, as Brama's lease document.
pub(crate) const INHERITED: &str = "JEDEN_LEASE";

/// The lease this process was started on, when its starter holds one: the
/// session runs on it and takes none of its own, because the starter's
/// proxy names the account on every request and relays no lease route.
/// Unset is no such lease; a value that is not Unicode or not a lease
/// document is refused with its cause, never read as no lease. An explicit
/// direct provider does not use a Brama lease and ignores this variable.
pub(crate) fn inherited() -> Result<Option<SessionLease>, String> {
    if crate::agent::credential::direct_provider().is_some()
        || crate::agent::credential::direct_refusal().is_some()
    {
        return Ok(None);
    }
    let raw = match std::env::var(INHERITED) {
        Ok(raw) => raw,
        Err(std::env::VarError::NotPresent) => return Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err(format!(
                "{INHERITED} is not Unicode, so the lease it hands this session cannot be read"
            ));
        }
    };
    serde_json::from_str(&raw).map(Some).map_err(|error| {
        format!("{INHERITED} does not hold Brama's lease document ({error}): {raw}")
    })
}

/// The register as Brama shows it to Jeden's agent: its own live leases, the
/// sessions each account carries, each provider's stated limit and the
/// refusal of each provider whose declaration states none.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Register {
    pub scope: Value,
    pub limits: BTreeMap<String, u64>,
    pub unstated: BTreeMap<String, Value>,
    pub leases: Vec<SessionLease>,
    pub counts: BTreeMap<String, u64>,
}

#[derive(Debug, Deserialize)]
struct Released {
    released: Vec<SessionLease>,
}

/// One signed request to Brama's lease routes. A refusal is Brama's whole
/// error document with its status, so the code, the message and the details
/// (each member's count, the acquisition's verdict) reach the reader as
/// Brama wrote them.
fn call(
    config: &ChatConfig,
    method: reqwest::Method,
    path: &str,
    body: Option<Value>,
) -> Result<Value, String> {
    let url = format!("{}{path}", config.url.trim_end_matches('/'));
    let operation = format!("{method} {url}");
    let body_text = match &body {
        Some(body) => serde_json::to_string(body)
            .map_err(|error| format!("{operation}: the request cannot be encoded: {error}"))?,
        None => String::new(),
    };
    let signed = agent_headers(&body_text, &config.agent_id, &config.secret)
        .map_err(|error| format!("{operation}: the request cannot be signed: {error}"))?;
    let client = crate::net::blocking_builder().build().map_err(|error| {
        format!(
            "{operation}: the HTTP client cannot be built: {}",
            crate::control_plane::transport::describe_reqwest(error)
        )
    })?;
    let mut request = client
        .request(method, &url)
        .bearer_auth(&config.bearer_token);
    for (name, value) in signed {
        request = request.header(name, value);
    }
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let response = request.body(body_text).send().map_err(|error| {
        format!(
            "{operation} was not answered: {}",
            crate::control_plane::transport::describe_reqwest(error)
        )
    })?;
    let status = response.status();
    let text = response.text().map_err(|error| {
        format!(
            "{operation} answered HTTP {} and its body cannot be read: {}",
            status.as_u16(),
            crate::control_plane::transport::describe_reqwest(error)
        )
    })?;
    if !status.is_success() {
        return Err(format!(
            "Brama refused {operation}: HTTP {}: {text}",
            status.as_u16()
        ));
    }
    serde_json::from_str(&text)
        .map_err(|error| format!("{operation} answered a body that is not JSON ({error}): {text}"))
}

fn answered<T: serde::de::DeserializeOwned>(answer: Value, what: &str) -> Result<T, String> {
    serde_json::from_value(answer.clone()).map_err(|error| {
        format!("Brama's {what} does not have the lease shape ({error}): {answer}")
    })
}

/// The register: this agent's live leases and each provider's limit.
pub fn register(config: &ChatConfig) -> Result<Register, String> {
    answered(
        call(config, reqwest::Method::GET, LEASES, None)?,
        "lease register",
    )
}

/// Take a lease for `session_id` on `provider`, or the one it already holds.
pub fn take(config: &ChatConfig, provider: &str, session_id: &str) -> Result<Taken, String> {
    let body = json!({"provider": provider, "session_id": session_id, "holder": HOLDER});
    answered(
        call(config, reqwest::Method::POST, LEASES, Some(body))?,
        "lease",
    )
}

/// Release the one lease `lease_id`. Releasing what is no longer live
/// releases nothing and is not an error.
pub fn release(config: &ChatConfig, lease_id: &str) -> Result<Vec<SessionLease>, String> {
    let path = format!("{LEASES}/{lease_id}");
    let released: Released = answered(
        call(config, reqwest::Method::DELETE, &path, None)?,
        "release",
    )?;
    Ok(released.released)
}

/// Release every live lease of `session_id` that this agent took.
pub fn release_session(config: &ChatConfig, session_id: &str) -> Result<Vec<SessionLease>, String> {
    let body = json!({"session_id": session_id});
    let released: Released = answered(
        call(config, reqwest::Method::DELETE, LEASES, Some(body))?,
        "release",
    )?;
    Ok(released.released)
}

/// What each leased account has left, keyed by the member's subscription id:
/// Brama's pool document (`GET /v1/subscription-pool`, as this agent sees
/// it) states `accounts.capacity` per account, and each member of an account
/// is keyed to its account's row. Only the members of live leases are kept,
/// because that is what `jeden lease list` and Jeden Desktop ask about. A
/// document without the object is answered as none, said by the caller.
pub fn capacity(
    config: &ChatConfig,
    leases: &[SessionLease],
) -> Result<Option<BTreeMap<String, Value>>, String> {
    let document = call(config, reqwest::Method::GET, POOL, None)?;
    let Some(accounts) = document
        .pointer("/accounts/capacity")
        .and_then(Value::as_array)
    else {
        return Ok(None);
    };
    let mut keyed = BTreeMap::new();
    for account in accounts {
        let members = account
            .get("members")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str);
        for member in members {
            if leases.iter().any(|lease| lease.subscription_id == member) {
                keyed.insert(member.to_owned(), account.clone());
            }
        }
    }
    Ok(Some(keyed))
}
