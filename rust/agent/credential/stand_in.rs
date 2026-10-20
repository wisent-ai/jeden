//! What Jeden uses for models when Brama does not serve it. Jeden works
//! without the other Wisent products: when Brama is not routed here, this
//! process holds no credential for it, or it refuses or does not answer
//! Jeden's catalog read, model work goes to the OpenAI-compatible provider
//! the operator keeps in Jeden's own token store (`jeden token add`), and
//! standard error says which provider and why. With no stored provider the
//! run refuses naming both Brama's cause and the empty store.
use std::sync::OnceLock;

use crate::control_plane::brama::BramaError;

/// The provider standing in for Brama in this process, and why.
pub(crate) struct StandIn {
    pub(crate) endpoint: String,
    pub(crate) cause: String,
}

static ENGAGED: OnceLock<StandIn> = OnceLock::new();

/// The stand-in this process already took, if it took one.
pub(crate) fn engaged() -> Option<&'static StandIn> {
    ENGAGED.get()
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

/// Use the first provider Jeden's token store holds instead of Brama, because
/// of `cause`, and say so once on standard error. Refused, naming `cause` and
/// why no provider stands in, when the store holds none or cannot be read.
pub(crate) fn engage(cause: &str) -> Result<&'static StandIn, String> {
    if let Some(engaged) = ENGAGED.get() {
        return Ok(engaged);
    }
    let stored = super::tokens::Store::open_existing()
        .and_then(|store| match store {
            Some(store) => store.list(),
            None => Ok(Vec::new()),
        })
        .map(|rows| rows.into_iter().map(|row| row.endpoint).next());
    let endpoint = match stored {
        Ok(Some(endpoint)) => endpoint,
        Ok(None) => {
            return Err(format!(
                "{cause}; no provider stands in for it: Jeden's token store holds no \
                 OpenAI-compatible provider (`jeden token add <label> --endpoint <url>`, key on stdin)"
            ))
        }
        Err(unreadable) => return Err(format!("{cause}; no provider stands in for it: {unreadable}")),
    };
    let engaged = ENGAGED.get_or_init(|| StandIn {
        endpoint,
        cause: cause.to_owned(),
    });
    eprintln!(
        "jeden: {}; using the OpenAI-compatible provider {} from Jeden's token store instead",
        engaged.cause, engaged.endpoint
    );
    Ok(engaged)
}
