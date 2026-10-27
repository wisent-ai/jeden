//! The hand sign-in that banks a subscription grant in Jeden's own gateway:
//! the gateway draws the provider's page and keeps the PKCE verifier, the
//! operator signs in and pastes the code, and the gateway exchanges it,
//! banks the member and stores the grant in the grants file.
use serde_json::Value;
use std::sync::LazyLock;

use super::{running, start, Gateway};

/// The client the console requests share: built from the crate's own
/// blocking builder, so a request that needs to stop early stops on
/// something that happened, never on a clock the library set.
static CONSOLE_CLIENT: LazyLock<Result<reqwest::blocking::Client, reqwest::Error>> =
    LazyLock::new(|| crate::net::blocking_builder().build());

/// The pool id of `provider`'s subscription of `account`: both folded to
/// lowercase letters, digits and dashes, joined by a dash, with the hex
/// digest of the raw names appended so distinct accounts that fold alike
/// (first.last and first-last) never share a member id.
fn member_id(provider: &str, account: &str) -> String {
    use sha2::{Digest, Sha256};
    let fold = |text: &str| -> String {
        text.trim()
            .to_lowercase()
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() {
                    character
                } else {
                    '-'
                }
            })
            .collect()
    };
    let digest = hex::encode(
        Sha256::new()
            .chain_update(provider.as_bytes())
            .chain_update(b"\0")
            .chain_update(account.as_bytes())
            .finalize(),
    );
    format!("{}-{}-{digest}", fold(provider), fold(account))
}

/// One console response, retaining its HTTP status through schema inspection.
fn console(
    gateway: &Gateway,
    path: &str,
    body: &Value,
) -> Result<(reqwest::StatusCode, Value), String> {
    let url = format!("{}{path}", gateway.endpoint);
    let response = CONSOLE_CLIENT
        .as_ref()
        .map_err(|error| {
            format!("Jeden's own gateway client for POST {path} cannot be built: {error}")
        })?
        .post(&url)
        .bearer_auth(&gateway.bearer)
        .json(body)
        .send()
        .map_err(|error| format!("Jeden's own gateway did not answer POST {path}: {error}"))?;
    let status = response.status();
    let text = response.text().map_err(|error| {
        format!("Jeden's own gateway's answer to POST {path} ({status}) cannot be read: {error}")
    })?;
    if !status.is_success() {
        return Err(format!(
            "Jeden's own gateway refused POST {path} ({status}): {text}"
        ));
    }
    serde_json::from_str(&text)
        .map(|document| (status, document))
        .map_err(|error| {
            format!("Jeden's own gateway answered POST {path} ({status}) with no JSON ({error}): {text}")
        })
}

/// Whether the pool this gateway serves already holds `member`: read from the
/// pool document the console answers, never assumed, because Brama banks a
/// new member only under an id no member holds.
fn held_member(gateway: &Gateway, member: &str) -> Result<bool, String> {
    let url = format!("{}/v1/subscription-pool", gateway.endpoint);
    let response = CONSOLE_CLIENT
        .as_ref()
        .map_err(|error| {
            format!(
                "Jeden's own gateway client for GET /v1/subscription-pool cannot be built: {error}"
            )
        })?
        .get(&url)
        .bearer_auth(&gateway.bearer)
        .send()
        .map_err(|error| {
            format!("Jeden's own gateway did not answer GET /v1/subscription-pool: {error}")
        })?;
    let status = response.status();
    let text = response
        .text()
        .map_err(|error| {
            format!("Jeden's own gateway's answer to GET /v1/subscription-pool ({status}) cannot be read: {error}")
        })?;
    if !status.is_success() {
        return Err(format!(
            "Jeden's own gateway refused GET /v1/subscription-pool ({status}): {text}"
        ));
    }
    let pool: Value = serde_json::from_str(&text).map_err(|error| {
        format!(
            "Jeden's own gateway answered GET /v1/subscription-pool ({status}) with no JSON ({error}): {text}"
        )
    })?;
    let members = pool["subscriptions"].as_array().ok_or_else(|| {
        format!("Jeden's own gateway answered GET /v1/subscription-pool ({status}) with no subscriptions list: {text}")
    })?;
    Ok(members
        .iter()
        .any(|member_row| member_row["id"].as_str() == Some(member)))
}

/// Begin a hand sign-in of the operator's own subscription of `provider`,
/// account `account`, into Jeden's own gateway: the gateway draws the
/// provider's page and keeps the PKCE verifier. A member the pool already
/// holds is signed in by its id alone, so a grant that expired or lost its
/// refresh token can be signed in again; a new member is named by provider
/// and account, which Brama accepts only for an id no member holds. Answers
/// the sign-in id and the page to open.
pub(crate) fn sign_in_begin(
    provider: &str,
    account: &str,
    reason: &str,
) -> Result<(String, String), String> {
    let gateway = start()?;
    let member = member_id(provider, account);
    let request = if held_member(&gateway, &member)? {
        serde_json::json!({ "by": "hand", "subscription_id": member, "reason": reason })
    } else {
        serde_json::json!({
            "by": "hand", "subscription_id": member, "provider": provider,
            "account": account, "reason": reason,
        })
    };
    let (status, page) = console(&gateway, "/v1/admin/subscription-pool/sign-in", &request)?;
    match (page["sign_in_id"].as_str(), page["url"].as_str()) {
        (Some(sign_in), Some(url)) => Ok((sign_in.to_owned(), url.to_owned())),
        _ => Err(format!(
            "Jeden's own gateway answered POST /v1/admin/subscription-pool/sign-in ({status}) without a sign-in id and a page: {page}"
        )),
    }
}

/// Complete sign-in `sign_in` with the `code` the page showed: the gateway
/// exchanges it, banks the member, stores the grant in Jeden's grants file
/// and proves it with one completion. The gateway's own verdict decides the
/// outcome: `signed_in` and `unchanged` are subscriptions this process can
/// use, and every other answer (a code the provider refused, a stored grant
/// its proof would not serve) is refused with the verdict's own detail, so
/// no failed sign-in is reported as a stored credential. Answers the
/// verdict.
pub(crate) fn sign_in_complete(sign_in: &str, code: &str) -> Result<Value, String> {
    let code = code.trim();
    if code.is_empty() {
        return Err(
            "no code was pasted: the sign-in is left open on Jeden's own gateway and ends with it"
                .into(),
        );
    }
    let gateway = running()?.ok_or_else(|| {
        format!(
            "sign-in {sign_in} was begun on a gateway this process no longer runs; begin it again"
        )
    })?;
    let path = format!("/v1/admin/subscription-pool/sign-in/{sign_in}");
    let (status, verdict) = console(&gateway, &path, &serde_json::json!({ "code": code }))?;
    match verdict.get("result").and_then(Value::as_str) {
        Some("signed_in") | Some("unchanged") => Ok(verdict),
        Some(other) => Err(format!(
            "Jeden's own gateway refused POST {path} ({status}; its result is {other}): {verdict}"
        )),
        None => Err(format!(
            "Jeden's own gateway answered POST {path} ({status}) without a result: {verdict}"
        )),
    }
}

/// `jeden token sign-in <provider> --account <email> --reason <text>`: begin
/// the sign-in, print the page, read the pasted code on standard input and
/// complete it. Answers the gateway's verdict.
pub(crate) fn sign_in(provider: &str, account: &str, reason: &str) -> Result<Value, String> {
    let (sign_in, url) = sign_in_begin(provider, account, reason)?;
    eprintln!("Open this page, sign in as {account}, and paste the code it shows here:\n{url}");
    let mut code = String::new();
    std::io::stdin().read_line(&mut code).map_err(|error| {
        format!("the code the page showed cannot be read from standard input: {error}")
    })?;
    sign_in_complete(&sign_in, &code)
}
