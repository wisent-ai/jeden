//! The request for a GitHub star: after the operator's fifth successful
//! prompt Jeden asks, once, for a star on its public repository. The request
//! is the `star-request` journey every Wisent product walks, run by the
//! shared onboarding runtime (`wisent_onboarding_client::ProductStarRequest`)
//! and kept per user in `~/.jeden/star-request.json`, so every surface
//! (terminal, `jeden run`, RPC and Jeden Desktop) counts into the same number
//! and the request is made once, wherever the fifth prompt was sent. The
//! threshold is the runtime's, the operator's number; the repository is this
//! package's `repository`, the catalog's `star_repository`.

pub(crate) mod command;

use wisent_onboarding_client::{ProductStarRequest, StarProduct, StarRequest, StarStatus};

fn request() -> Result<ProductStarRequest, String> {
    let product = StarProduct::from_package("jeden", "Jeden", env!("CARGO_PKG_REPOSITORY"))
        .map_err(|error| error.to_string())?;
    let file = crate::slash::common::dirs_home().join(".jeden/star-request.json");
    Ok(ProductStarRequest::at(product, file))
}

/// The request's current state, without changing it.
pub(crate) fn status() -> Result<StarStatus, String> {
    request()?.status().map_err(|error| error.to_string())
}

/// Never show the request again, until a reset.
pub(crate) fn dismiss() -> Result<StarStatus, String> {
    request()?.dismiss().map_err(|error| error.to_string())
}

/// Start over: the count returns to zero and the request is made again
/// after the threshold.
pub(crate) fn reset() -> Result<StarStatus, String> {
    request()?.reset().map_err(|error| error.to_string())
}

/// Count one successful prompt from a caller that cannot fail on the
/// bookkeeping: the request, when this prompt makes it; a request that
/// cannot be kept is said on standard error, naming the cause, and the
/// prompt's own answer stands.
pub(crate) fn observe_prompt_reporting() -> Option<StarRequest> {
    match request().and_then(|request| request.observe().map_err(|error| error.to_string())) {
        Ok(made) => made,
        Err(cause) => {
            eprintln!("jeden: this prompt is not counted toward the GitHub star request: {cause}");
            None
        }
    }
}

/// The request in words, in the workspace's interface language.
pub(crate) fn request_text(lang: &str, request: &StarRequest) -> String {
    crate::cli::i18n::tr(lang, "star.request")
        .replace("{prompts}", &request.actions.to_string())
        .replace("{repository}", &request.repository)
}
