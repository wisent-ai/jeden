//! Background goal-lifecycle classification via Oko's local qualified model.
//!
//! Each classified user prompt is sent to the loopback OpenAI-compatible
//! endpoint served by `com.wisent.compute.service.oko-goal-lifecycle`
//! (`mlx_lm.server`). Everything here is fail-open: when the service is
//! unreachable the first probe caches the verdict for the process lifetime and
//! every later call is a fast no-op, so a Jeden turn never blocks on Oko.

use serde_json::{json, Value};
use std::env;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use url::{Position, Url};

/// Model label recorded in ledger events; also the served-id fallback when
/// `GET {base}/v1/models` does not name the loaded model.
pub const LIFECYCLE_MODEL_LABEL: &str = "oko-goal-lifecycle-v1";

/// Verbatim copy of the lifecycle system prompt. Source of truth:
/// `training/lifecycle-model/lifecycle-system-prompt.txt` in
/// `/Users/lukaszbartoszcze/Documents/CodingProjects/Wisent/transcript-label-trainer`
/// (the same file Oko vendors in `LocalGoalLifecycleClassifier.systemPrompt`).
/// Keep the .txt byte-identical to that file; do not add headers to it.
const SYSTEM_PROMPT: &str = include_str!("goal_lifecycle_prompt.txt");

/// Verbatim copy of the goal-completion judge prompt. The lifecycle contract
/// above deliberately refuses to close a goal on an assistant's say-so; this
/// judge is the operator-requested counterpart that reads the assistant's own
/// final report and keeps the goal open whenever that report names leftover,
/// blocked, or deferred work.
const JUDGE_PROMPT: &str = include_str!("goal_completion_judge_prompt.txt");

const DEFAULT_COMPLETIONS_URL: &str = "http://127.0.0.1:11439/v1/chat/completions";
const PROBE_TIMEOUT: Duration = Duration::from_millis(1500);
const CLASSIFY_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleAction {
    StartGoal,
    ContinueCurrent,
    FinishGoal,
    Ignore,
}

impl LifecycleAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::StartGoal => "startGoal",
            Self::ContinueCurrent => "continueCurrent",
            Self::FinishGoal => "finishGoal",
            Self::Ignore => "ignore",
        }
    }
}

/// Strictly parsed model verdict. Unknown `action` or `lifecycle_evidence`
/// values reject the whole reply (treated as "service said nothing").
#[derive(Debug, Clone)]
pub struct LifecycleDecision {
    pub action: LifecycleAction,
    pub goal_ref: String,
    pub lifecycle_evidence: String,
}

pub struct LifecycleRequest {
    pub prompt: String,
    pub session_id: String,
    pub turn_index: u64,
    /// Current active goal objective, when goal mode holds one.
    pub goal_objective: Option<String>,
}

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
    let client = reqwest::blocking::Client::builder()
        .timeout(PROBE_TIMEOUT)
        .build()
        .ok()?;
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
/// timeout, malformed reply — yields `None` and the turn proceeds unchanged.
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
    let client = reqwest::blocking::Client::builder()
        .timeout(CLASSIFY_TIMEOUT)
        .build()
        .ok()?;
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

/// Ask the local model whether the active goal is genuinely complete, given
/// the assistant's final answer for this turn. `Some(true)` means finished;
/// any transport, parse, or contract violation reads as "no verdict".
pub fn judge_completion(objective: &str, assistant_final: &str) -> Option<bool> {
    let endpoint = endpoint()?;
    let envelope = json!({
        "goal_title": objective,
        "assistant_final": [assistant_final.chars().take(6000).collect::<String>()],
        "user_after": [],
    });
    let body = json!({
        "model": endpoint.model,
        "messages": [
            { "role": "system", "content": JUDGE_PROMPT },
            { "role": "user", "content": envelope.to_string() },
        ],
        "temperature": 0,
        "max_tokens": 96,
        "stream": false,
        "chat_template_kwargs": { "enable_thinking": false },
    });
    let client = reqwest::blocking::Client::builder()
        .timeout(CLASSIFY_TIMEOUT)
        .build()
        .ok()?;
    let response: Value = client
        .post(&endpoint.completions_url)
        .json(&body)
        .send()
        .ok()?
        .error_for_status()
        .ok()?
        .json()
        .ok()?;
    let content = response
        .get("choices")?
        .get(0)?
        .get("message")?
        .get("content")?
        .as_str()?
        .trim();
    let verdict: Value = serde_json::from_str(content).ok()?;
    match verdict.get("verdict").and_then(Value::as_str) {
        Some("finished") => Some(true),
        Some("open") => Some(false),
        _ => None,
    }
}

/// Resolve a title for a freshly started goal: `transcript-lake goal title
/// --stdin --json` when the executable is available, otherwise the prompt's
/// first line trimmed to 100 characters.
pub fn resolve_goal_title(prompt: &str) -> String {
    if let Some(title) = transcript_lake_title(prompt) {
        return title;
    }
    let first_line = prompt.lines().next().unwrap_or("").trim();
    let mut title: String = first_line.chars().take(100).collect();
    if title.is_empty() {
        title = "New goal".to_string();
    }
    title
}

fn transcript_lake_title(prompt: &str) -> Option<String> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let executable = find_transcript_lake()?;
    let mut child = Command::new(executable)
        .args(["goal", "title", "--stdin", "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    child.stdin.take()?.write_all(prompt.as_bytes()).ok()?;
    let output = child.wait_with_output().ok()?;
    if !output.status.success() {
        return None;
    }
    let value: Value = serde_json::from_slice(&output.stdout).ok()?;
    value
        .get("goal")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|goal| !goal.is_empty())
        .map(str::to_string)
}

fn find_transcript_lake() -> Option<PathBuf> {
    let mut directories: Vec<PathBuf> = env::var_os("PATH")
        .map(|path| env::split_paths(&path).collect())
        .unwrap_or_default();
    if let Some(home) = env::var_os("HOME") {
        directories.push(Path::new(&home).join(".local/bin"));
    }
    directories.push(PathBuf::from("/opt/homebrew/bin"));
    directories.push(PathBuf::from("/usr/local/bin"));
    directories
        .into_iter()
        .map(|directory| directory.join("transcript-lake"))
        .find(|candidate| candidate.is_file())
}

/// A `(text, status)` sink for goal-lifecycle events, shared rather than
/// borrowed because the background threads below outlive the turn that
/// started them.
pub(crate) type GoalEventSink = Arc<dyn Fn(&str, &str) + Send + Sync>;

/// Background classification for one user turn. Never blocks the caller:
/// spawns a thread that classifies the prompt, records the `goal_lifecycle`
/// ledger event, emits the RPC `goal` session event when a sink exists, and —
/// only when `/goal auto on` — actuates goal mode state. Returns the thread's
/// handle so the end-of-turn completion judge can order itself after this
/// classification; a fast turn would otherwise let the classifier's
/// `startGoal` land after the judge and re-open a goal the judge just closed.
pub(crate) fn spawn_turn_classification(
    cwd: PathBuf,
    prompt: String,
    session_dir: PathBuf,
    turn_index: u64,
    goal_event: Option<GoalEventSink>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let state = crate::slash::read_mode_state(&cwd);
        let goal_objective = Some(state.goal.objective.trim())
            .filter(|objective| state.goal.enabled && !objective.is_empty())
            .map(str::to_string);
        let session_id = session_dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "unknown".to_string());
        let Some(decision) = classify(&LifecycleRequest {
            prompt: prompt.clone(),
            session_id,
            turn_index,
            goal_objective: goal_objective.clone(),
        }) else {
            return;
        };
        let _ = crate::cli::sessions::append_ledger_entry(
            &session_dir,
            crate::agent::now_stamp(),
            "goal_lifecycle",
            json!({
                "action": decision.action.as_str(),
                "goal_ref": decision.goal_ref,
                "lifecycle_evidence": decision.lifecycle_evidence,
                "goal": goal_objective,
                "model": LIFECYCLE_MODEL_LABEL,
            }),
        );
        match decision.action {
            LifecycleAction::StartGoal => {
                let title = resolve_goal_title(&prompt);
                if state.goal.auto {
                    let objective = title.clone();
                    let _ = crate::slash::mutate_mode_state(&cwd, move |state| {
                        state.goal.objective = objective;
                        state.goal.enabled = true;
                        state.goal.paused = false;
                        Ok(())
                    });
                }
                if let Some(emit) = &goal_event {
                    emit(&title, "active");
                }
            }
            LifecycleAction::FinishGoal => {
                if state.goal.auto {
                    // Mirror `/goal drop`.
                    let _ = crate::slash::mutate_mode_state(&cwd, |state| {
                        state.goal.enabled = false;
                        state.goal.paused = false;
                        state.goal.objective.clear();
                        state.goal.budget = None;
                        Ok(())
                    });
                }
                if let (Some(emit), Some(objective)) = (&goal_event, &goal_objective) {
                    emit(objective, "done");
                }
            }
            LifecycleAction::ContinueCurrent | LifecycleAction::Ignore => {}
        }
    })
}

/// Background completion judgement after an agent turn yields its final
/// answer. Never blocks the caller and is fail-open like classification.
/// Orders itself strictly after this turn's prompt classification via
/// `classification`, then re-reads goal state, so a fast turn cannot have
/// the classifier re-open a goal this judge closes. When the judge reads
/// the assistant's report as a genuine completion — past tense, no named
/// leftovers — the active goal closes: ledger event, RPC `goal` event with
/// status "done", and (under `/goal auto on`) the same state reset
/// `/goal drop` performs. A report naming unfinished work leaves the goal
/// open and records the open verdict. Note for stdio `jeden rpc` clients:
/// the per-prompt event forwarder unsubscribes at the terminal result, so
/// this judge's late "done" event reaches long-lived subscribers (desktop,
/// daemon replay) but not a plain request/response stdio client; the
/// ledger and mode state remain the source of truth.
pub(crate) fn spawn_completion_judgement(
    cwd: PathBuf,
    session_dir: PathBuf,
    assistant_final: String,
    goal_event: Option<GoalEventSink>,
    classification: Option<std::thread::JoinHandle<()>>,
) {
    std::thread::spawn(move || {
        if let Some(handle) = classification {
            let _ = handle.join();
        }
        let state = crate::slash::read_mode_state(&cwd);
        let objective = state.goal.objective.trim().to_string();
        if !state.goal.enabled || objective.is_empty() {
            return;
        }
        let Some(finished) = judge_completion(&objective, &assistant_final) else {
            return;
        };
        let _ = crate::cli::sessions::append_ledger_entry(
            &session_dir,
            crate::agent::now_stamp(),
            "goal_lifecycle",
            json!({
                "action": if finished { "finishGoal" } else { "continueCurrent" },
                "judge": "completion",
                "goal": objective,
                "model": LIFECYCLE_MODEL_LABEL,
            }),
        );
        if !finished {
            return;
        }
        if state.goal.auto {
            // Mirror `/goal drop`.
            let _ = crate::slash::mutate_mode_state(&cwd, |state| {
                state.goal.enabled = false;
                state.goal.paused = false;
                state.goal.objective.clear();
                state.goal.budget = None;
                Ok(())
            });
        }
        if let Some(emit) = &goal_event {
            emit(&objective, "done");
        }
    });
}
