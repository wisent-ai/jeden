//! Local credential management over the authenticated Jeden RPC connection.
//! Responses contain identifiers and diagnostics, never secret material.
use crate::agent::credential::{self, tokens::Store};
use serde_json::{json, Value};

pub(crate) type Answer = Result<Value, (&'static str, String)>;
fn failed(error: String) -> (&'static str, String) {
    ("credential_error", error)
}
fn required<'a>(params: &'a Value, name: &str) -> Result<&'a str, (&'static str, String)> {
    params
        .get(name)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            (
                "invalid_params",
                format!("token request requires non-empty {name}"),
            )
        })
}

fn list() -> Answer {
    let tokens = match Store::open_existing().map_err(failed)? {
        Some(store) => store.list().map_err(failed)?,
        None => Vec::new(),
    };
    Ok(json!({ "tokens": tokens }))
}

pub(crate) fn request(method: &str, params: &Value) -> Answer {
    match method {
        "token/list" => list(),
        "token/status" => {
            let mut value = list()?;
            match credential::direct_provider() {
                Some((endpoint, key)) => {
                    let rows = value["tokens"]
                        .as_array()
                        .expect("token/list returns an array of tokens");
                    let selected = if key.is_none() {
                        json!(rows
                            .iter()
                            .find(|row| row["active"].as_bool() == Some(true)
                                && row["endpoint"].as_str() == Some(endpoint.as_str()))
                            .and_then(|row| row["id"].as_str()))
                    } else {
                        Value::Null
                    };
                    let stored = rows
                        .iter()
                        .any(|row| row["endpoint"].as_str() == Some(endpoint.as_str()));
                    value["route"] = json!("direct");
                    value["endpoint"] = json!(endpoint);
                    value["keySource"] = json!(if key.is_some() {
                        "environment"
                    } else if stored {
                        "local-token-list"
                    } else {
                        "none"
                    });
                    value["selected"] = selected;
                }
                None => {
                    if let Some(cause) = credential::direct_refusal() {
                        value["route"] = json!("direct");
                        value["endpoint"] = Value::Null;
                        value["keySource"] = json!("refused");
                        value["selected"] = Value::Null;
                        value["routeRefusal"] = json!(cause);
                    } else {
                        value["route"] = json!("brama");
                        value["endpoint"] = json!(credential::brama_route().url);
                        value["origin"] = json!(credential::route_origin());
                        let (secret, bearer) = credential::ensure();
                        value["signingCredential"] = json!(secret.as_str());
                        value["gatewayBearer"] = json!(bearer.as_str());
                        value["keySource"] = json!(bearer.as_str());
                        value["selected"] = Value::Null;
                        value["signingRefusal"] = json!(secret.refusal());
                        value["bearerRefusal"] = json!(bearer.refusal());
                    }
                }
            }
            Ok(value)
        }
        "token/add" => {
            let label = required(params, "label")?;
            let endpoint = required(params, "endpoint")?;
            let secret = required(params, "token")?;
            let row = Store::add(label, endpoint, secret).map_err(failed)?;
            serde_json::to_value(row).map_err(|error| failed(format!("encode token row: {error}")))
        }
        "token/remove" => {
            let id = required(params, "id")?;
            Store::remove_existing(id).map_err(failed)?;
            Ok(json!({ "removed": id }))
        }
        _ => Err((
            "method_not_found",
            format!("unknown token method: {method}"),
        )),
    }
}
