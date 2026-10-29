//! Jeden's model endpoint and process-owned credentials. Stado may locate
//! Brama's address; model credentials are never acquired from Skarbiec.
use std::env;
use std::sync::{LazyLock, OnceLock};

mod directory;
pub(crate) mod private_gateway;
pub(crate) mod stand_in;
pub(crate) mod tokens;

pub(crate) const SECRET: &str = "WISENT_APP_AGENT_AUTH_SECRET";
pub(crate) const BEARER: &str = "BRAMA_TOKEN";
pub(crate) const DIRECT_URL: &str = "JEDEN_MODEL_ENDPOINT";
pub(crate) const DIRECT_KEY: &str = "JEDEN_MODEL_KEY";
pub(crate) const ADDRESS: &str = "BRAMA_URL";
const DIRECTORY_ROUTE: (&str, &str) = ("brama", "jeden");

/// A route variable as the process holds it: unset or blank is no value; a
/// value that is not Unicode is a refusal naming the variable, never no value.
fn route_variable(name: &str) -> Result<Option<String>, String> {
    match env::var(name) {
        Ok(value) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else if trimmed.len() == value.len() {
                Ok(Some(value))
            } else {
                Ok(Some(trimmed.to_owned()))
            }
        }
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => {
            Err(format!("{name} cannot be read: value is not Unicode"))
        }
    }
}

/// The direct endpoint and, when one is set, its key.
fn direct_variables() -> Result<Option<(String, Option<String>)>, String> {
    let Some(url) = route_variable(DIRECT_URL)? else {
        return Ok(None);
    };
    let endpoint =
        tokens::canonical_endpoint(&url).map_err(|error| format!("{DIRECT_URL}: {error}"))?;
    Ok(Some((endpoint, route_variable(DIRECT_KEY)?)))
}
pub(crate) fn bearer_token() -> Result<Option<String>, String> {
    route_variable(BEARER)
}

/// The variable naming the agent Jeden's signed requests speak for.
pub(crate) const AGENT_ID: &str = "WISENT_APP_AGENT_ID";

/// The agent Jeden signs Brama and Weles requests as, the one order every
/// reader shares: `WISENT_APP_AGENT_ID`, then the configuration's `agentId`
/// (`configuration`). Neither stated is refused naming both: an agent filled
/// in by Jeden signed every request as an agent whose grant this secret may
/// not hold, and Brama answered each one as forbidden.
pub(crate) fn agent_id(configuration: Option<&str>) -> Result<String, String> {
    if let Some(stated) = route_variable(AGENT_ID)? {
        return Ok(stated);
    }
    match configuration.map(str::trim).filter(|stated| !stated.is_empty()) {
        Some(configured) => Ok(configured.to_owned()),
        None => Err(format!(
            "no agent is stated for signed requests: set {AGENT_ID} or the configuration's agentId to the agent Brama grants {SECRET} to (jeden run \"/setup agent-id <id>\")"
        )),
    }
}

/// Why this process refuses an explicit direct provider route whose
/// endpoint or key cannot be read, or whose endpoint is invalid.
static DIRECT_REFUSED: OnceLock<String> = OnceLock::new();

/// An explicit direct endpoint turns Brama off. Otherwise the provider that
/// stands in for Brama, once `stand_in::engage` took one because Brama does
/// not serve this process, and said so on standard error: a lost Brama route
/// never sends model work elsewhere silently. An unreadable direct variable
/// selects no provider and no Brama either: its refusal is kept for
/// `brama_route`, which every model call, doctor and Desktop read it through.
pub(crate) fn direct_provider() -> Option<(String, Option<String>)> {
    stand_in::selected_provider().map(|provider| (provider.endpoint, provider.key))
}

/// What serves this process in Brama's place, once a stand-in was engaged:
/// Jeden's own gateway (on subscription grants signed in by hand) or a
/// stored provider. The surfaces that report the route read this, so a
/// bearer Jeden minted for its own gateway is never reported as the
/// process environment's key.
pub(crate) fn standing_in() -> Option<&'static str> {
    stand_in::engaged().map(|standing| match standing.stored {
        Some(_) => "stored-provider",
        None => "own-gateway",
    })
}

/// The refusal left by a direct provider variable read in this process.
pub(crate) fn direct_refusal() -> Option<&'static str> {
    DIRECT_REFUSED.get().map(String::as_str)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Source {
    Environment,
    Stado,
    DirectRefused(String),
    EnvironmentRefused(String),
    Refused(String),
    NotNeeded,
}
impl Source {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Environment => "environment",
            Self::Stado => "stado",
            Self::DirectRefused(_) | Self::EnvironmentRefused(_) | Self::Refused(_) => "refused",
            Self::NotNeeded => "not-needed",
        }
    }
    pub(crate) fn refusal(&self) -> Option<&str> {
        match self {
            Self::DirectRefused(said) | Self::EnvironmentRefused(said) | Self::Refused(said) => {
                Some(said)
            }
            _ => None,
        }
    }
}

/// Brama credentials come only from the caller's environment. Its signing
/// secret and gateway bearer are distinct; the runtime names either missing
/// value before dispatch and reports any HTTP refusal from Brama itself.
pub(crate) fn ensure() -> &'static (Source, Source) {
    static RESOLVED: LazyLock<(Source, Source)> = LazyLock::new(|| {
        if direct_provider().is_some() {
            return (Source::NotNeeded, Source::NotNeeded);
        }
        let source = |name: &str| match env::var(name) {
            Ok(value) if !value.trim().is_empty() => Source::Environment,
            Ok(_) | Err(env::VarError::NotPresent) => {
                Source::Refused(format!("{name} is not configured in this process"))
            }
            Err(env::VarError::NotUnicode(_)) => {
                Source::Refused(format!("{name} cannot be read: value is not Unicode"))
            }
        };
        (source(SECRET), source(BEARER))
    });
    &RESOLVED
}

static FILE_SUPPLIED: OnceLock<Vec<String>> = OnceLock::new();
pub(crate) fn record_file_supplied(names: Vec<String>) {
    // main records the names once, from the one credentials file it loads; a
    // second recording would carry no other file, so the first stands.
    let _already_recorded = FILE_SUPPLIED.set(names);
}
fn file_supplied(name: &str) -> bool {
    FILE_SUPPLIED
        .get()
        .is_some_and(|names| names.iter().any(|known| known == name))
}

pub(crate) struct Route {
    pub(crate) url: Option<String>,
    pub(crate) source: Source,
    pub(crate) replaced_written_address: bool,
    directory: Option<directory::Provenance>,
}

/// An exported address wins; otherwise Stado's current directory route wins
/// over a written file copy. A direct provider needs no Brama route.
pub(crate) fn brama_route() -> &'static Route {
    static ROUTE: LazyLock<Route> = LazyLock::new(|| {
        let direct = direct_provider();
        if let Some(said) = DIRECT_REFUSED.get() {
            return Route {
                url: None,
                source: Source::DirectRefused(said.clone()),
                replaced_written_address: false,
                directory: None,
            };
        }
        if direct.is_some() {
            return Route {
                url: None,
                source: Source::NotNeeded,
                replaced_written_address: false,
                directory: None,
            };
        }
        let current = match route_variable(ADDRESS) {
            Ok(value) => value,
            Err(said) => {
                return Route {
                    url: None,
                    source: Source::EnvironmentRefused(said),
                    replaced_written_address: false,
                    directory: None,
                };
            }
        };
        if current.is_some() && !file_supplied(ADDRESS) {
            return Route {
                url: current,
                source: Source::Environment,
                replaced_written_address: false,
                directory: None,
            };
        }
        match directory::resolve() {
            Ok((url, provenance)) => {
                unsafe { env::set_var(ADDRESS, &url) };
                Route {
                    replaced_written_address: current
                        .as_ref()
                        .is_some_and(|written| *written != url),
                    url: Some(url),
                    source: Source::Stado,
                    directory: Some(provenance),
                }
            }
            Err(said) => Route {
                url: current,
                source: Source::Refused(said),
                replaced_written_address: false,
                directory: None,
            },
        }
    });
    &ROUTE
}
pub(crate) fn brama_url() -> Option<String> {
    brama_route().url.clone()
}
pub(crate) fn route_origin() -> String {
    directory::origin(brama_route())
}
