use super::{Route, Source, ADDRESS, DIRECTORY_ROUTE};
use std::process::Command;

#[derive(serde::Deserialize)]
pub(crate) struct Provenance {
    placed_on: String,
    from: String,
    verified: bool,
}

#[derive(serde::Deserialize)]
struct Address {
    url: String,
    service: String,
    consumer: String,
    #[serde(flatten)]
    provenance: Provenance,
}

/// Read the declared route without probing the model gateway.
pub(super) fn resolve() -> Result<(String, Provenance), String> {
    let (service, consumer) = DIRECTORY_ROUTE;
    let command = format!(
        "stado service directory connect {service} --consumer {consumer} --no-verify --json"
    );
    let output = stado_wait::output(Command::new("stado").args([
        "service",
        "directory",
        "connect",
        service,
        "--consumer",
        consumer,
        "--no-verify",
        "--json",
    ]))
    .map_err(|error| {
        format!(
            "the Stado CLI could not be started to run `{command}`: {error}. Install \
             Stado, or carry {ADDRESS} in this process's environment"
        )
    })?;
    if !output.status.success() {
        return Err(format!(
            "`{command}` ended {}; stdout: {}; stderr: {}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let mut answer: Address = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("`{command}` printed an invalid route answer: {error}"))?;
    if answer.service != service || answer.consumer != consumer {
        return Err(format!(
            "`{command}` answered for service {} consumer {}, not service {service} consumer {consumer}",
            answer.service, answer.consumer
        ));
    }
    let trimmed = answer.url.trim();
    if trimmed.is_empty() {
        return Err(format!("`{command}` answered with an empty url"));
    }
    if trimmed.len() != answer.url.len() {
        answer.url = trimmed.to_owned();
    }
    Ok((answer.url, answer.provenance))
}

pub(super) fn origin(route: &Route) -> String {
    let mut origin = match (&route.source, &route.url, route.replaced_written_address) {
        (Source::Environment, _, _) => {
            "address from BRAMA_URL in the process environment".to_string()
        }
        (Source::Stado, _, true) => {
            "address from Stado's service directory (consumer jeden); an environment file held a different address, which that route replaced".to_string()
        },
        (Source::Stado, _, false) => {
            "address from Stado's service directory (consumer jeden)".to_string()
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
    if let Some(provenance) = &route.directory {
        let (service, consumer) = DIRECTORY_ROUTE;
        origin.push_str(&format!(
            "; directory selected service {service}, consumer {consumer}, placed_on {}, from {}, verified {}",
            provenance.placed_on, provenance.from, provenance.verified
        ));
    }
    origin
}
