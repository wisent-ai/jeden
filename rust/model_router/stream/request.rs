//! The HTTP part of a streamed model call. Credential rejection is resolved
//! before any model output is exposed; an exhausted pool retains its last cause.
use super::*;
use std::io::{BufRead, BufReader};

pub(crate) fn spawn_openai_stream_adapter(
    config: &ChatConfig,
    request_route: &super::super::transport::RequestRoute<'_>,
    body_text: String,
    lease_headers: Vec<(&'static str, String)>,
    sender: Sender<WireMessage>,
) -> Result<(), AttemptError> {
    let signed = agent_headers(&body_text, &config.agent_id, &config.secret)
        .map_err(AttemptError::permanent)?;
    let url = format!(
        "{}/v1/chat/completions",
        request_route.endpoint.trim_end_matches('/')
    );
    let endpoint = request_route.endpoint.to_string();
    let bearer = request_route.bearer.to_string();
    let use_store = !config.via_brama && bearer.is_empty();
    std::thread::Builder::new()
        .name("model-stream-adapter".into())
        .spawn(move || {
            if let Err(error) = send_request(
                Destination {
                    url: &url,
                    endpoint: &endpoint,
                    bearer: &bearer,
                    use_store,
                },
                &body_text,
                &signed,
                &lease_headers,
                &sender,
            ) {
                let _unreceived = sender.send(WireMessage::Network(error));
            }
        })
        .map_err(|error| {
            AttemptError::permanent(format!("cannot start model stream adapter: {error}"))
        })?;
    Ok(())
}

/// Where one streamed call goes and how it is authorized: the completions
/// URL, the endpoint it hangs off, the bearer, and whether the bearer is read
/// from the token store instead.
struct Destination<'a> {
    url: &'a str,
    endpoint: &'a str,
    bearer: &'a str,
    use_store: bool,
}

fn send_request(
    destination: Destination<'_>,
    body: &str,
    signed: &[(&'static str, String)],
    lease_headers: &[(&'static str, String)],
    sender: &Sender<WireMessage>,
) -> Result<(), String> {
    let Destination {
        url,
        endpoint,
        bearer,
        use_store,
    } = destination;
    let client = crate::net::blocking_builder()
        .build()
        .map_err(crate::control_plane::transport::describe_reqwest)?;
    let mut store = if use_store {
        crate::agent::credential::tokens::Store::open_existing()?
    } else {
        None
    };
    let mut tried = Vec::new();
    let mut selected = match &mut store {
        Some(store) => store.next(endpoint, &tried)?,
        None => None,
    };
    loop {
        let token = match &selected {
            Some(token) => token.secret.as_str(),
            None => bearer,
        };
        let mut request = client
            .post(url)
            .header("content-type", "application/json")
            .header("accept", "text/event-stream");
        if !token.is_empty() {
            request = request.bearer_auth(token);
        }
        for (name, value) in signed.iter().chain(lease_headers) {
            request = request.header(*name, value.as_str());
        }
        let response = request
            .body(body.to_owned())
            .send()
            .map_err(crate::control_plane::transport::describe_reqwest)?;
        let status = response.status();
        let content_type = match response.headers().get("content-type") {
            Some(value) => value
                .to_str()
                .map(str::to_owned)
                .map_err(|error| format!("read model content-type: {error}")),
            None => Ok(String::new()),
        };
        let retry_after = match response.headers().get("retry-after") {
            Some(value) => value
                .to_str()
                .map_err(|error| format!("read model retry-after: {error}"))
                .and_then(|text| parse_retry_after(text).map(Some)),
            None => Ok(None),
        };
        let switch = matches!(
            status,
            reqwest::StatusCode::UNAUTHORIZED
                | reqwest::StatusCode::FORBIDDEN
                | reqwest::StatusCode::PAYMENT_REQUIRED
                | reqwest::StatusCode::TOO_MANY_REQUESTS
        );
        if switch {
            if let (Some(failed), Some(store)) = (&selected, &mut store) {
                let body = response
                    .text()
                    .map(|body| body.replace(failed.secret.as_str(), "[REDACTED]"))
                    .map_err(crate::control_plane::transport::describe_reqwest);
                let cause = match &body {
                    Ok(body) => format!("HTTP {status}: {body}"),
                    Err(error) => format!(
                        "HTTP {status}: read model refusal body failed: {}",
                        error.replace(failed.secret.as_str(), "[REDACTED]")
                    ),
                };
                store.failure(&failed.id, &failed.secret, "POST model request", &cause)?;
                tried.push(failed.id.clone());
                if let Some(next) = store.next(endpoint, &tried)? {
                    selected = Some(next);
                    continue;
                }
                match body {
                    Ok(body) => {
                        let content_type =
                            content_type.map_err(|error| format!("{cause}; {error}"))?;
                        let retry_after =
                            retry_after.map_err(|error| format!("{cause}; {error}"))?;
                        let _unreceived = sender.send(WireMessage::Headers {
                            status: status.as_u16(),
                            content_type,
                            retry_after,
                        });
                        let _unreceived = sender.send(WireMessage::FullBody(Ok(body)));
                    }
                    Err(error) => {
                        let _unreceived = sender.send(WireMessage::Network(format!(
                            "HTTP {status}: read model refusal body failed: {}",
                            error.replace(failed.secret.as_str(), "[REDACTED]")
                        )));
                    }
                }
                return Ok(());
            }
        }
        let content_type = content_type.map_err(|error| format!("HTTP {status}: {error}"))?;
        let retry_after = retry_after.map_err(|error| format!("HTTP {status}: {error}"))?;
        let streaming = status.is_success() && content_type.contains("event-stream");
        if sender
            .send(WireMessage::Headers {
                status: status.as_u16(),
                content_type,
                retry_after,
            })
            .is_err()
        {
            return Ok(());
        }
        if !streaming {
            let result = response
                .text()
                .map(|body| {
                    if token.is_empty() {
                        body
                    } else {
                        body.replace(token, "[REDACTED]")
                    }
                })
                .map_err(crate::control_plane::transport::describe_reqwest);
            let _unreceived = sender.send(WireMessage::FullBody(result));
            return Ok(());
        }
        for line in BufReader::new(response).lines() {
            let line = line
                .map(|line| {
                    if token.is_empty() || !line.contains(token) {
                        line
                    } else {
                        line.replace(token, "[REDACTED]")
                    }
                })
                .map_err(|error| error.to_string());
            if sender.send(WireMessage::Line(line)).is_err() {
                return Ok(());
            }
        }
        let _unreceived = sender.send(WireMessage::Eof);
        return Ok(());
    }
}
