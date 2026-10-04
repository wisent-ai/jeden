//! Where Jeden's own Brama credential comes from when the environment does
//! not carry it.
//!
//! The harness holds no credential store and writes no secret to disk, so the
//! signing secret and the router bearer have to arrive from the product that
//! owns credential delivery: Stado, reading Skarbiec. A checked-in shell
//! script is not where that capability lives; it lives here, inside the
//! product, so a configured workstation never answers `BRAMA_URL is required`
//! with no way to satisfy it.
//!
//! Two roles, each read one field at a time through `stado credentials get
//! --role`: the item playing `wisent-app-agent` holds the HMAC signing secret
//! every request is signed with in `value`, and the item playing
//! `jeden-model-router` holds the gateway bearer in `token`. No item is named:
//! the vault says which item plays each role, so replacing or renaming an item
//! changes nothing here. Neither value ever reaches a command line — `stado`
//! writes it to stdout, this module keeps it in the process environment, and
//! nothing writes it to disk.

use std::env;
use std::process::Command;
use std::sync::LazyLock;

/// The signing secret's environment name.
pub(crate) const SECRET: &str = "WISENT_APP_AGENT_AUTH_SECRET";
/// The gateway bearer's environment name.
pub(crate) const BEARER: &str = "BRAMA_TOKEN";
/// The role whose item holds the signing secret, and the field.
pub(crate) const SECRET_ROLE: (&str, &str) = ("wisent-app-agent", "value");
/// The role whose item holds the gateway bearer, and the field.
const BEARER_ROLE: (&str, &str) = ("jeden-model-router", "token");
/// An OpenAI-compatible provider that answers `/v1/models` and
/// `/v1/chat/completions` itself, named in place of Brama by a user who does
/// not run Brama.
pub(crate) const DIRECT_URL: &str = "JEDEN_MODEL_ENDPOINT";
/// That provider's API key, sent as the bearer; a local server that takes
/// none leaves it unset.
pub(crate) const DIRECT_KEY: &str = "JEDEN_MODEL_KEY";

/// The provider named by `JEDEN_MODEL_ENDPOINT`, when the user chose one.
///
/// Only an explicit setting turns Brama off: a workstation that lost Brama
/// still fails with Brama's own sentence instead of silently calling someone
/// else.
pub(crate) fn direct_provider() -> Option<(String, Option<String>)> {
    let read = |name: &str| {
        env::var(name)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    };
    read(DIRECT_URL).map(|url| (url, read(DIRECT_KEY)))
}

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
    /// `JEDEN_MODEL_ENDPOINT` names a provider that is called without Brama,
    /// so no Brama credential is needed and Stado was not asked.
    NotNeeded,
}

impl Source {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Environment => "environment",
            Self::Stado => "stado",
            Self::Refused(_) => "refused",
            Self::NotNeeded => "not-needed",
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

/// The `stado` command group that reads credential fields.
const CREDENTIALS_GROUP: &str = "credentials";

/// Read one field of the item that plays `role` through the Stado CLI.
///
/// `stado credentials get --role <role> --field <field>` prints the value and
/// nothing else. A non-zero exit carries Stado's own sentence, which is the
/// sentence a caller needs: an absent binary, an unauthorized consumer and a
/// role no item plays are three different problems, and Stado already words
/// them apart.
fn field_from_stado(role: &str, field: &str) -> Result<String, String> {
    let output = Command::new("stado")
        .arg(CREDENTIALS_GROUP)
        .args(["get", "--role", role, "--field", field])
        .output()
        .map_err(|error| {
            format!(
                "the Stado CLI could not be started to read role {role} field {field}: {error}. \
                 Install Stado, or carry {SECRET} and BRAMA_URL in this process's environment"
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
            "stado credentials get --role {role} --field {field} exited {}: {said}",
            output.status.code().unwrap_or(-1)
        ));
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if value.is_empty() {
        return Err(format!(
            "stado credentials get --role {role} --field {field} printed nothing, so that field is \
             empty in Skarbiec and no request can be signed with it"
        ));
    }
    Ok(value)
}

/// Put one credential in this process's environment, from Stado when the
/// environment does not already carry it.
fn resolve_one(variable: &str, (role, field): (&str, &str)) -> Source {
    if env::var(variable)
        .ok()
        .is_some_and(|value| !value.trim().is_empty())
    {
        return Source::Environment;
    }
    match field_from_stado(role, field) {
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
        if direct_provider().is_some() {
            return (Source::NotNeeded, Source::NotNeeded);
        }
        (
            resolve_one(SECRET, SECRET_ROLE),
            resolve_one(BEARER, BEARER_ROLE),
        )
    });
    &RESOLVED
}
