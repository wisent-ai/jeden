//! Where Jeden's own Brama credential comes from when the environment does
//! not carry it.
//!
//! The harness holds no credential store and writes no secret to disk, so the
//! signing secret and the router bearer have to arrive from the product that
//! owns credential delivery: Stado, reading Skarbiec. Until 2026-09-06 a
//! checked-in shell script did that, and `Remove the scripts directories`
//! deleted it on the operator's instruction that every capability lives in the
//! product it belongs to. Nothing replaced it: `bin/jeden-rust` still exec'd
//! the deleted file, `/setup` still told the reader to run it, and on a
//! configured workstation `jeden run` answered `BRAMA_URL is required` with no
//! way to satisfy it. This is that capability, inside the product.
//!
//! Two items, each read one field at a time through `stado secrets get`:
//! `agent:wisent-app/value` is the HMAC signing secret every request is signed
//! with, and `jeden-model-router/token` is the gateway bearer. Neither value
//! ever reaches a command line — `stado` writes it to stdout, this module
//! keeps it in the process environment, and nothing writes it to disk.

use std::env;
use std::process::Command;
use std::sync::LazyLock;

/// The signing secret's environment name.
pub(crate) const SECRET: &str = "WISENT_APP_AGENT_AUTH_SECRET";
/// The gateway bearer's environment name.
pub(crate) const BEARER: &str = "BRAMA_TOKEN";
/// The Skarbiec item and field that hold the signing secret.
const SECRET_ITEM: (&str, &str) = ("agent:wisent-app", "value");
/// The Skarbiec item and field that hold the gateway bearer.
const BEARER_ITEM: (&str, &str) = ("jeden-model-router", "token");

/// What one resolution attempt did, for `/setup`, `doctor` and the run's own
/// refusal sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Source {
    /// The value was already in the environment; Stado was not asked.
    Environment,
    /// Stado answered, and the value is now in this process's environment.
    Stado,
    /// Stado could not answer, and this is what it said.
    Refused(String),
}

impl Source {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Environment => "environment",
            Self::Stado => "stado",
            Self::Refused(_) => "refused",
        }
    }

    /// Stado's own sentence, for a caller that has to explain the refusal.
    pub(crate) fn refusal(&self) -> Option<&str> {
        match self {
            Self::Refused(said) => Some(said.as_str()),
            _ => None,
        }
    }
}

/// Read one field of one Skarbiec item through the Stado CLI.
///
/// `stado secrets get <item> --field <field>` prints the value and nothing
/// else. A non-zero exit carries Stado's own sentence, which is the sentence a
/// caller needs: an absent binary, an unauthorized consumer and an item that
/// does not exist are three different problems, and Stado already words them
/// apart.
fn field_from_stado(item: &str, field: &str) -> Result<String, String> {
    let output = Command::new("stado")
        .args(["secrets", "get", item, "--field", field])
        .output()
        .map_err(|error| {
            format!(
                "the Stado CLI could not be started to read {item}/{field}: {error}. Install \
                 Stado, or carry {SECRET} and BRAMA_URL in this process's environment"
            )
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let said = if stderr.is_empty() {
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        } else {
            stderr
        };
        return Err(format!(
            "stado secrets get {item} --field {field} exited {}: {said}",
            output.status.code().unwrap_or(-1)
        ));
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if value.is_empty() {
        return Err(format!(
            "stado secrets get {item} --field {field} printed nothing, so that field is empty in \
             Skarbiec and no request can be signed with it"
        ));
    }
    Ok(value)
}

/// Put one credential in this process's environment, from Stado when the
/// environment does not already carry it.
fn resolve_one(variable: &str, (item, field): (&str, &str)) -> Source {
    if env::var(variable)
        .ok()
        .is_some_and(|value| !value.trim().is_empty())
    {
        return Source::Environment;
    }
    match field_from_stado(item, field) {
        Ok(value) => {
            // This runs before the model runtime is built, while nothing else
            // in the process reads or writes the environment.
            unsafe { env::set_var(variable, value) };
            Source::Stado
        }
        Err(said) => Source::Refused(said),
    }
}

/// Resolve the signing secret and the gateway bearer once for this process.
///
/// The bearer is optional — Brama requires it only on the deployments that
/// declare one — so a refusal there is reported and does not stop a run, while
/// a missing signing secret is what makes every request unsignable.
///
/// Every control-plane secret in this binary is read by name through
/// `SecretRef::resolve`, and the Brama client resolves its bearer before it
/// signs a request, so the first read of any name is early enough for the
/// signature that follows it. Calling this more than once costs nothing: the
/// answer is computed on the first call and reused, so `stado` is asked at
/// most once per process however many requests a turn makes.
pub(crate) fn ensure() -> &'static (Source, Source) {
    static RESOLVED: LazyLock<(Source, Source)> = LazyLock::new(|| {
        (
            resolve_one(SECRET, SECRET_ITEM),
            resolve_one(BEARER, BEARER_ITEM),
        )
    });
    &RESOLVED
}
