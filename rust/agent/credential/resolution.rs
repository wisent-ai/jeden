//! Where Jeden's model work goes when Jeden asks Stado. Jeden never dials
//! Brama through Stado's service directory itself: `stado capabilities
//! resolve inference --consumer jeden --json` answers Brama's route when
//! Brama answers Jeden, or the OpenAI-compatible endpoint the operator
//! declared in its place, with the model it is asked for and the role of the
//! item holding its key. When no alternative answers, the refusal names each
//! one's product, route and cause.
use super::{Route, Source, ADDRESS, CONSUMER};
use std::process::Command;

use serde_json::Value;

/// How Stado proved the alternative it handed out.
pub(crate) struct Provenance {
    proven_at: String,
    proven_now: bool,
}

/// An OpenAI-compatible endpoint Stado hands out in Brama's place: its
/// provider name, its origin, the model it is asked for and the role of the
/// credential-store item holding its key, when it takes one.
#[derive(Clone)]
pub(crate) struct Alternative {
    pub(crate) provider: String,
    pub(crate) endpoint: String,
    pub(crate) model: String,
    pub(crate) key_role: Option<String>,
    /// Why each alternative before it did not answer, as Stado said it.
    pub(crate) passed_over: String,
}

/// What Stado's resolution answered.
pub(super) enum Resolved {
    /// Brama answers Jeden at this route.
    Brama(String, Provenance),
    /// Brama does not; this endpoint answers in its place.
    InPlace(Alternative),
}

#[derive(serde::Deserialize)]
struct Answer {
    family: String,
    consumer: String,
    provider: String,
    route: String,
    model: Option<String>,
    key_role: Option<String>,
    proven_at: String,
    proven_now: bool,
    #[serde(default)]
    unreachable: Vec<Unreachable>,
}

#[derive(serde::Deserialize)]
struct Unreachable {
    product: String,
    route: String,
    cause: String,
}

fn named(items: &[Unreachable]) -> String {
    items
        .iter()
        .map(|item| format!("{} at {}: {}", item.product, item.route, item.cause))
        .collect::<Vec<_>>()
        .join("; ")
}

/// Ask Stado which inference alternative answers Jeden. `failed_brama`
/// reports that Brama, handed out before, refused this process, so Stado
/// proves it again before handing it out once more.
pub(super) fn resolve(failed_brama: bool) -> Result<Resolved, String> {
    let mut arguments = vec![
        "capabilities",
        "resolve",
        "inference",
        "--consumer",
        CONSUMER,
    ];
    if failed_brama {
        arguments.extend(["--failed", "brama"]);
    }
    arguments.push("--json");
    let command = format!("stado {}", arguments.join(" "));
    let output = stado_wait::output(Command::new("stado").args(&arguments)).map_err(|error| {
        format!(
            "the Stado CLI could not be started to run `{command}`: {error}. Install \
             Stado, or carry {ADDRESS} in this process's environment"
        )
    })?;
    if !output.status.success() {
        // The refusal envelope names every alternative with its cause.
        let unreachable = [&output.stdout, &output.stderr]
            .into_iter()
            .filter_map(|bytes| serde_json::from_slice::<Value>(bytes).ok())
            .find_map(|envelope| {
                serde_json::from_value::<Vec<Unreachable>>(
                    envelope["details"]["unreachable"].clone(),
                )
                .ok()
            })
            .filter(|items| !items.is_empty());
        return Err(match unreachable {
            Some(items) => format!(
                "no inference alternative answers {CONSUMER} (`{command}`): {}",
                named(&items)
            ),
            None => format!(
                "`{command}` ended {}; stdout: {}; stderr: {}",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ),
        });
    }
    let answer: Answer = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("`{command}` printed an invalid answer: {error}"))?;
    if answer.family != "inference" || answer.consumer != CONSUMER {
        return Err(format!(
            "`{command}` answered family {} for consumer {}, not inference for {CONSUMER}",
            answer.family, answer.consumer
        ));
    }
    let route = answer.route.trim();
    if route.is_empty() {
        return Err(format!("`{command}` answered with an empty route"));
    }
    if answer.provider == "brama" {
        return Ok(Resolved::Brama(
            route.to_owned(),
            Provenance {
                proven_at: answer.proven_at,
                proven_now: answer.proven_now,
            },
        ));
    }
    let endpoint = super::tokens::canonical_endpoint(route)
        .map_err(|error| format!("{} at {route}: {error}", answer.provider))?;
    let model = answer
        .model
        .filter(|model| !model.trim().is_empty())
        .ok_or_else(|| {
            format!(
                "{} at {route} answers in Brama's place but declares no model: set \
                 capabilities.inference.endpoint.model",
                answer.provider
            )
        })?;
    Ok(Resolved::InPlace(Alternative {
        provider: answer.provider,
        endpoint,
        model,
        key_role: answer.key_role.filter(|role| !role.trim().is_empty()),
        passed_over: named(&answer.unreachable),
    }))
}

/// The key of the endpoint Stado handed out, read as the role it names
/// (`stado credentials get --role ROLE --field token`); none when it names
/// no role.
pub(super) fn key(alternative: &Alternative) -> Result<Option<String>, String> {
    let Some(role) = &alternative.key_role else {
        return Ok(None);
    };
    let command = format!("stado credentials get --role {role} --field token");
    let output = stado_wait::output(Command::new("stado").args([
        "credentials",
        "get",
        "--role",
        role,
        "--field",
        "token",
    ]))
    .map_err(|error| format!("`{command}` could not be started: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "{} at {} answers in Brama's place, but `{command}` ended {}: {}",
            alternative.provider,
            alternative.endpoint,
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let key = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if key.is_empty() {
        return Err(format!("`{command}` answered an empty key"));
    }
    Ok(Some(key))
}

pub(super) fn origin(route: &Route) -> String {
    let mut origin = match (&route.source, &route.url, route.replaced_written_address) {
        (Source::Environment, _, _) => {
            "address from BRAMA_URL in the process environment".to_string()
        }
        (Source::Stado, _, true) => {
            "address from Stado's capability resolution (inference, consumer jeden); an environment file held a different address, which that route replaced".to_string()
        },
        (Source::Stado, _, false) => {
            "address from Stado's capability resolution (inference, consumer jeden)".to_string()
        }
        (Source::DirectRefused(said), _, _) =>
            format!("direct provider route refused: {said}"),
        (Source::EnvironmentRefused(said), _, _) =>
            format!("BRAMA_URL in the process environment refused: {said}"),
        (Source::Refused(said), Some(_), _) => {
            format!("address from an environment file, because Stado did not route it: {said}")
        }
        (Source::Refused(said), None, _) => format!("Stado did not route it: {said}"),
        (Source::NotNeeded, _, _) => {
            "JEDEN_MODEL_ENDPOINT names a provider called without Brama".to_string()
        }
    };
    if let Some(provenance) = &route.provenance {
        origin.push_str(&format!(
            "; Stado proved brama at {} ({})",
            provenance.proven_at,
            if provenance.proven_now {
                "proven now"
            } else {
                "a kept proof"
            }
        ));
    }
    origin
}
