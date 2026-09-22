//! Asking the local qualified model what a prompt means for the goal a
//! session is pursuing, and refusing to believe anything it is not sure of.
//!
//! Split out of `goal_lifecycle/mod.rs`, which had grown past the module line
//! cap.

use super::{LifecycleAction, LifecycleDecision, LifecycleRequest, LIFECYCLE_MODEL_LABEL};
use serde_json::{json, Value};
use std::env;
use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};
use url::{Position, Url};

/// Verbatim copy of the lifecycle system prompt. Source of truth:
/// `training/lifecycle-model/lifecycle-system-prompt.txt` in the
/// transcript-label-trainer checkout, the same file Oko vendors. Keep the
/// .txt byte-identical to that file; do not add headers to it.
const SYSTEM_PROMPT: &str = include_str!("prompt.txt");

const DEFAULT_COMPLETIONS_URL: &str = "http://127.0.0.1:11439/v1/chat/completions";

struct Endpoint {
    completions_url: String,
    model: String,
}

/// One-time availability probe. Resolves the completions URL (env override
/// `JEDEN_LIFECYCLE_MODEL_URL`, loopback-only), asks `GET {base}/v1/models`
/// for the served model id, and caches both the id and the reachability
/// verdict for the process lifetime.
static ENDPOINT: LazyLock<Option<Endpoint>> = LazyLock::new(|| {
    let raw = env::var("JEDEN_LIFECYCLE_MODEL_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_COMPLETIONS_URL.to_string());
    let url = Url::parse(raw.trim()).ok()?;
    if !is_loopback(&url) {
        return None;
    }
    let base = url[..Position::BeforePath].to_string();
    let client = crate::net::blocking_builder().build().ok()?;
    let models: Value = client
        .get(format!("{base}/v1/models"))
        .send()
        .ok()?
        .error_for_status()
        .ok()?
        .json()
        .ok()?;
    let model = models
        .pointer("/data/0/id")
        .and_then(Value::as_str)
        .unwrap_or(LIFECYCLE_MODEL_LABEL)
        .to_string();
    Some(Endpoint {
        completions_url: url.to_string(),
        model,
    })
});

fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        Some(url::Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        None => false,
    }
}

fn endpoint() -> Option<&'static Endpoint> {
    ENDPOINT.as_ref()
}

/// UTC RFC3339 timestamp and calendar day via Howard Hinnant's
/// civil-from-days; no date crate is available here. The envelope's
/// `local_day` therefore uses the UTC day.
fn utc_now_strings() -> (String, String) {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year_of_era = yoe;
    let doy = doe - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let local_day = format!("{year:04}-{month:02}-{day:02}");
    let timestamp = format!(
        "{local_day}T{:02}:{:02}:{:02}Z",
        rem / 3_600,
        (rem / 60) % 60,
        rem % 60
    );
    (timestamp, local_day)
}

fn build_envelope(request: &LifecycleRequest) -> Value {
    let (timestamp, local_day) = utc_now_strings();
    let member_id = env::var("USER").unwrap_or_else(|_| "unknown".to_string());
    let candidates = match request.goal_objective.as_deref() {
        Some(objective) => json!([
            {
                "ref": "C1",
                "title": objective,
                "score": 0.9,
                "same_session": true,
                "is_last_member_goal": true,
            },
            { "ref": "NEW_GOAL", "title": "Create a new goal", "score": 0 },
        ]),
        None => json!([
            { "ref": "NEW_GOAL", "title": "Create a new goal", "score": 0 },
        ]),
    };
    json!({
        "prompt_id": format!("{}-{}", request.session_id, request.turn_index),
        "member_id": member_id,
        "provider": "jeden",
        "session_id": request.session_id,
        "turn_index": request.turn_index,
        "timestamp": timestamp,
        "local_day": local_day,
        "text": request.prompt,
        "recent_session_prompts": [],
        "recent_member_prompts": [],
        "candidates": candidates,
    })
}

fn parse_decision(content: &str) -> Option<LifecycleDecision> {
    let value: Value = serde_json::from_str(content.trim()).ok()?;
    let object = value.as_object()?;
    let action = match object.get("action")?.as_str()? {
        "startGoal" => LifecycleAction::StartGoal,
        "continueCurrent" => LifecycleAction::ContinueCurrent,
        "finishGoal" => LifecycleAction::FinishGoal,
        "ignore" => LifecycleAction::Ignore,
        _ => return None,
    };
    let goal_ref = object.get("goal_ref")?.as_str()?.to_string();
    let lifecycle_evidence = match object.get("lifecycle_evidence")?.as_str()? {
        evidence @ ("none" | "explicit_open" | "explicit_completion") => evidence.to_string(),
        _ => return None,
    };
    Some(LifecycleDecision {
        action,
        goal_ref,
        lifecycle_evidence,
    })
}

/// Classify one user prompt. Any failure — service down, non-loopback URL,
/// malformed reply — yields `None` and the turn proceeds unchanged.
pub fn classify(request: &LifecycleRequest) -> Option<LifecycleDecision> {
    let endpoint = endpoint()?;
    let envelope =
        serde_json::to_string(&build_envelope(request)).unwrap_or_else(|_| "{}".to_string());
    let body = json!({
        "model": endpoint.model,
        "messages": [
            { "role": "system", "content": SYSTEM_PROMPT },
            { "role": "user", "content": envelope },
        ],
        "temperature": 0,
        "max_tokens": 96,
        "stream": false,
        "chat_template_kwargs": { "enable_thinking": false },
    });
    let client = crate::net::blocking_builder().build().ok()?;
    let response: Value = client
        .post(&endpoint.completions_url)
        .json(&body)
        .send()
        .ok()?
        .error_for_status()
        .ok()?
        .json()
        .ok()?;
    parse_decision(response.pointer("/choices/0/message/content")?.as_str()?)
}
