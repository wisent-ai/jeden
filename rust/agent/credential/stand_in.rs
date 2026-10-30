//! What Jeden uses for models when Brama does not serve it. Jeden works
//! without the other Wisent products: when Brama is not routed here, this
//! process holds no credential for it, or it refuses or does not answer
//! Jeden's catalog read, model work goes first to the OpenAI-compatible
//! endpoint Stado's capability resolution hands out in Brama's place (`stado
//! capabilities resolve inference --consumer jeden`, asked for its own model
//! with the key of the role it names); else to the provider the operator
//! keeps in Jeden's own token store (`jeden token add`); with none there and
//! subscription grants of Jeden's own (`private_gateway`), to the standalone
//! Brama this process starts on those grants. Standard error says which and
//! why. With none the run refuses naming Brama's cause, every alternative
//! Stado named, the empty store and the absent grants.
use std::sync::OnceLock;

use crate::control_plane::brama::BramaError;

/// The stand-in this process took for Brama, and why Brama does not serve
/// it. The route it serves is resolved when a caller asks, so Jeden's own
/// gateway is started again when its child ended, never answered from a
/// stale address.
pub(crate) struct StandIn {
    /// The endpoint of a stored OpenAI-compatible provider, whose key the
    /// token store selects per request; none for Jeden's own gateway and for
    /// the endpoint Stado resolved.
    pub(crate) stored: Option<String>,
    /// The endpoint Stado's capability resolution handed out in Brama's
    /// place, with the model it is asked for and its key.
    pub(crate) resolved: Option<Resolved>,
    pub(crate) cause: String,
}

/// The endpoint Stado hands out in Brama's place, as the stand-in serves it.
pub(crate) struct Resolved {
    pub(crate) provider: String,
    pub(crate) endpoint: String,
    pub(crate) model: String,
    pub(crate) key: Option<String>,
}

static ENGAGED: OnceLock<StandIn> = OnceLock::new();

/// The stand-in this process already took, if it took one.
pub(crate) fn engaged() -> Option<&'static StandIn> {
    ENGAGED.get()
}

/// The selected endpoint, credential and lifecycle source from one resolution.
/// Only a gateway this process owns follows replacement children at dispatch.
pub(crate) struct SelectedProvider {
    pub(crate) endpoint: String,
    pub(crate) key: Option<String>,
    pub(crate) own_gateway: bool,
}

/// Select an explicit provider before an engaged stand-in, preserving the
/// lifecycle source alongside the pair instead of comparing a later child.
pub(crate) fn selected_provider() -> Option<SelectedProvider> {
    let selected = super::direct_variables().and_then(|explicit| match explicit {
        Some((endpoint, key)) => Ok(Some(SelectedProvider {
            endpoint,
            key,
            own_gateway: false,
        })),
        None => selected_stand_in(),
    });
    match selected {
        Ok(provider) => provider,
        Err(cause) => {
            // The Brama route this refusal feeds is resolved once per process
            // (brama_route's LazyLock), so the first refusal is the one it says.
            let _already_refused = super::DIRECT_REFUSED.set(cause);
            None
        }
    }
}

/// The route the stand-in serves now: the endpoint Stado resolved with its
/// key, a stored provider's endpoint with no bearer, or Jeden's own gateway
/// with the bearer it accepts (started again when its child ended, so no
/// turn is sent to a dead address). None when no stand-in was engaged;
/// refused when the gateway will not come up.
fn selected_stand_in() -> Result<Option<SelectedProvider>, String> {
    let Some(standing) = ENGAGED.get() else {
        return Ok(None);
    };
    if let Some(resolved) = &standing.resolved {
        return Ok(Some(SelectedProvider {
            endpoint: resolved.endpoint.clone(),
            key: resolved.key.clone(),
            own_gateway: false,
        }));
    }
    match &standing.stored {
        Some(endpoint) => Ok(Some(SelectedProvider {
            endpoint: endpoint.clone(),
            key: None,
            own_gateway: false,
        })),
        None => {
            let gateway = super::private_gateway::start().map_err(|refused| {
                format!("Jeden's own gateway no longer serves this process: {refused}")
            })?;
            Ok(Some(SelectedProvider {
                endpoint: gateway.endpoint,
                key: Some(gateway.bearer),
                own_gateway: true,
            }))
        }
    }
}

/// The model the endpoint Stado resolved in Brama's place is asked for;
/// none for every other route, whose model the run selects.
pub(crate) fn resolved_model() -> Option<&'static str> {
    ENGAGED
        .get()
        .and_then(|standing| standing.resolved.as_ref())
        .map(|resolved| resolved.model.as_str())
}

/// Why Brama cannot be asked at all from this process, before any request:
/// no address, or a credential it signs or authorizes with is missing.
pub(crate) fn brama_unconfigured() -> Option<String> {
    let route = super::brama_route();
    if route.url.is_none() {
        return Some(match route.source.refusal() {
            Some(cause) => format!("Brama is not routed to this process: {cause}"),
            None => "Brama is not routed to this process: no BRAMA_URL and no Stado route".into(),
        });
    }
    let (secret, bearer) = super::ensure();
    secret
        .refusal()
        .or(bearer.refusal())
        .map(|cause| format!("Brama cannot be asked: {cause}"))
}

/// Whether a catalog read's failure means Brama does not serve this process:
/// it was not reached, refused the request, or failed. A model Brama does not
/// know and a cancelled read are answers about the request, not about Brama.
pub(crate) fn brama_not_serving(error: &BramaError) -> bool {
    matches!(
        error,
        BramaError::Unconfigured
            | BramaError::Configuration(_)
            | BramaError::Transport(_)
            | BramaError::Http { .. }
            | BramaError::RateLimited { .. }
    )
}

/// The endpoint Stado's capability resolution hands out in Brama's place:
/// the one `brama_route` kept when Stado never handed Brama out, else asked
/// again reporting that Brama, handed out before, does not serve this
/// process (`--failed brama`). `Err` names why Stado hands out none Jeden
/// can use.
fn resolved_in_place() -> Result<Resolved, String> {
    let route = super::brama_route();
    let alternative = match &route.in_place {
        Some(alternative) => alternative.clone(),
        None => match super::resolution::resolve(route.url.is_some())? {
            super::resolution::Resolved::InPlace(alternative) => alternative,
            super::resolution::Resolved::Brama(url, _) => {
                return Err(format!(
                    "Stado hands out Brama at {url} again, which does not serve this process"
                ))
            }
        },
    };
    let key = super::resolution::key(&alternative)?;
    Ok(Resolved {
        provider: alternative.provider,
        endpoint: alternative.endpoint,
        model: alternative.model,
        key,
    })
}

/// Use the endpoint Stado resolves in Brama's place, else the first provider
/// Jeden's token store holds, else Jeden's own gateway on its subscription
/// grants, because of `cause`, and say so once on standard error. Refused,
/// naming `cause`, why Stado handed out nothing Jeden can use and why
/// nothing else stands in, when none is there or one cannot be read or
/// started.
pub(crate) fn engage(cause: &str) -> Result<&'static StandIn, String> {
    if let Some(engaged) = ENGAGED.get() {
        return Ok(engaged);
    }
    let unresolved = match resolved_in_place() {
        Ok(resolved) => {
            let said = format!(
                "{} at {} with model {}, which Stado resolves in Brama's place",
                resolved.provider, resolved.endpoint, resolved.model
            );
            let engaged = ENGAGED.get_or_init(|| StandIn {
                stored: None,
                resolved: Some(resolved),
                cause: cause.to_owned(),
            });
            eprintln!("jeden: {}; using {said} instead", engaged.cause);
            return Ok(engaged);
        }
        Err(unresolved) => unresolved,
    };
    let cause = format!("{cause}; Stado resolves no alternative Jeden can use: {unresolved}");
    let stored = super::tokens::Store::open_existing()
        .and_then(|store| match store {
            Some(store) => store.list(),
            None => Ok(Vec::new()),
        })
        .map(|rows| rows.into_iter().map(|row| row.endpoint).next());
    let (stored, said) = match stored {
        Ok(Some(endpoint)) => {
            let said = format!("the OpenAI-compatible provider {endpoint} from Jeden's token store");
            (Some(endpoint), said)
        }
        Ok(None) => match super::private_gateway::holds_grants() {
            Ok(true) => {
                let gateway = super::private_gateway::start().map_err(|refused| {
                    format!("{cause}; Jeden's own gateway on its subscription grants did not start: {refused}")
                })?;
                let said = format!(
                    "Jeden's own gateway at {} on its subscription grants ({})",
                    gateway.endpoint,
                    super::private_gateway::grants_file().display()
                );
                (None, said)
            }
            Ok(false) => {
                return Err(format!(
                    "{cause}; no provider stands in for it: Jeden's token store holds no \
                     OpenAI-compatible provider (`jeden token add <label> --endpoint <url>`, key on stdin) \
                     and Jeden holds no subscription grant of its own ({}: \
                     jeden token sign-in PROVIDER --account EMAIL --reason TEXT signs one in)",
                    super::private_gateway::grants_file().display()
                ))
            }
            Err(unreadable) => return Err(format!("{cause}; no provider stands in for it: {unreadable}")),
        },
        Err(unreadable) => return Err(format!("{cause}; no provider stands in for it: {unreadable}")),
    };
    let engaged = ENGAGED.get_or_init(|| StandIn {
        stored,
        resolved: None,
        cause,
    });
    eprintln!("jeden: {}; using {said} instead", engaged.cause);
    Ok(engaged)
}
