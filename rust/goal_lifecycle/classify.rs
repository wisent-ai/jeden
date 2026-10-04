//! Asking the lifecycle model, through Brama, what a prompt means for the goal
//! a session is pursuing, and refusing to believe anything it is not sure of.
//!
//! Split out of `goal_lifecycle/mod.rs`, which had grown past the module line
//! cap.

use super::{LifecycleAction, LifecycleDecision, LifecycleRequest};
use crate::model_router::{chat_completion, ChatConfig};
use serde_json::{json, Value};
use std::env;
use std::time::{SystemTime, UNIX_EPOCH};

/// The Brama alias the lifecycle model is declared under on this machine. No
/// alias is built in: a machine that declares none has no classifier, and the
/// turn proceeds unchanged.
pub(super) const ALIAS_VARIABLE: &str = "JEDEN_LIFECYCLE_MODEL_ALIAS";

pub(super) fn alias() -> Option<String> {
    env::var(ALIAS_VARIABLE)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Verbatim copy of the lifecycle system prompt. Source of truth:
/// `training/lifecycle-model/lifecycle-system-prompt.txt` in the
/// transcript-label-trainer checkout, the same file Oko vendors. Keep the
/// .txt byte-identical to that file; do not add headers to it.
const SYSTEM_PROMPT: &str = include_str!("prompt.txt");

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

/// Classify one user prompt through Brama under the declared alias. No alias
/// declared answers `Ok(None)`; a refused call or an answer that is not a
/// decision answers why, for the ledger, and the turn proceeds unchanged.
pub fn classify(router: &ChatConfig, request: &LifecycleRequest) -> Result<Option<LifecycleDecision>, String> {
    let Some(alias) = alias() else {
        return Ok(None);
    };
    let envelope = serde_json::to_string(&build_envelope(request)).map_err(|error| error.to_string())?;
    let mut router = router.clone();
    router.model = alias.clone();
    router.fallbacks.clear();
    let messages = vec![
        json!({ "role": "system", "content": SYSTEM_PROMPT }),
        json!({ "role": "user", "content": envelope }),
    ];
    let completion = chat_completion(&router, messages, Some(96), &[])
        .map_err(|error| format!("Brama refused the lifecycle alias {alias}: {error}"))?;
    parse_decision(&completion.content)
        .map(Some)
        .ok_or_else(|| format!("the lifecycle alias {alias} answered no decision: {}", completion.content.chars().take(200).collect::<String>()))
}
