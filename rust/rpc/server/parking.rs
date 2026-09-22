//! Parking a `jeden rpc` that has nothing to do.
//!
//! Stado places every Jeden Desktop tab as `stado workload attach
//! jeden-session`, and the attach holds the kind's declared reservation for
//! exactly as long as this process lives. On 2026-09-22 eight such tabs held
//! 16 cores and 32 GiB of a 12-core laptop for 26 hours at 0.0% CPU: seven
//! resumed sessions had stopped at their first model call with `model router
//! 403` recorded as their completion blocker, the eighth was an empty tab,
//! and the laptop refused every fleet build with `reservations_exhausted`.
//!
//! Only this process knows whether a turn, a tool call, a question to the
//! operator or a completion continuation is in flight, so the decision is
//! made here and nowhere outside. When Stado declares a park time for the
//! kind it hands it over as `JEDEN_PARK_AFTER_SECONDS`; without that setting
//! nothing ever parks, which is what every other client of `jeden rpc` gets.
//! A process with no prompt in flight, no session with an active request, and
//! no frame from its client for that long sends one `parked` event and ends
//! through the ordinary shutdown path, which disposes its sessions to their
//! ledgers. A process whose every open session carries a completion blocker
//! parks after one quiet check, because a blocker is not work. Stado's attach
//! then sees its runtime end and releases the reservation; the session comes
//! back whole with `--resume`.
//!
//! The check is a periodic schedule that looks at the process; it never
//! interrupts anything. A turn that runs for an hour is work in flight for
//! that hour, and nothing here ends it.

use super::{read_frame, ServerState};
use crate::sdk::AgentSession;
use serde_json::{json, Value};
use std::io::BufRead;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// The setting Stado's workload declaration fills from the kind's
/// `park_after_seconds`.
const PARK_AFTER_SETTING: &str = "JEDEN_PARK_AFTER_SECONDS";

/// How often a parkable process looks at itself. Short enough that a blocked
/// session gives its reservation back within a quarter of a minute, long
/// enough that an idle process costs nothing to keep checking.
const CHECK_EVERY_SECONDS: u64 = 15;

pub(super) struct ParkPolicy {
    after: Duration,
    check_every: Duration,
}

impl ParkPolicy {
    /// The declared park time, or none. A value that is present and not a
    /// whole number of seconds above zero refuses the process before it
    /// answers `ready`: a declaration that says something unreadable must not
    /// quietly become "never park".
    pub(super) fn from_environment() -> Result<Option<Self>, String> {
        let Some(raw) = std::env::var_os(PARK_AFTER_SETTING) else {
            return Ok(None);
        };
        let raw = raw.to_string_lossy().into_owned();
        let seconds = raw
            .trim()
            .parse::<u64>()
            .ok()
            .filter(|seconds| *seconds > 0)
            .ok_or_else(|| {
                format!("{PARK_AFTER_SETTING} must be a whole number of seconds above zero, got {raw:?}")
            })?;
        Ok(Some(Self {
            after: Duration::from_secs(seconds),
            check_every: Duration::from_secs(seconds.min(CHECK_EVERY_SECONDS)),
        }))
    }
}

/// What the serving loop receives: a client frame, the client closing its
/// stream, or the moment to look at whether this process should park.
pub(super) enum Inbound {
    Frame(Result<Vec<u8>, String>),
    Closed,
    Check,
}

/// One channel carrying the client's frames and, when a park time is
/// declared, the periodic check. The loop that serves requests is the only
/// reader, so a check and a frame are never handled at the same time.
pub(super) fn inbox<R>(mut input: R, policy: Option<&ParkPolicy>) -> Result<Receiver<Inbound>, String>
where
    R: BufRead + Send + 'static,
{
    let (sender, receiver) = mpsc::channel();
    let frames = sender.clone();
    thread::spawn(move || loop {
        let inbound = match read_frame(&mut input) {
            Ok(Some(frame)) => Inbound::Frame(Ok(frame)),
            Ok(None) => {
                let _ = frames.send(Inbound::Closed);
                return;
            }
            Err(error) => Inbound::Frame(Err(error)),
        };
        if frames.send(inbound).is_err() {
            return;
        }
    });
    if let Some(policy) = policy {
        schedule_checks(sender, policy.check_every)?;
    }
    Ok(receiver)
}

/// A schedule that posts `Check` into the inbox at a fixed period until the
/// serving loop stops reading it.
fn schedule_checks(sender: Sender<Inbound>, every: Duration) -> Result<(), String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .map_err(|error| format!("cannot start the park check schedule: {error}"))?;
    thread::spawn(move || {
        runtime.block_on(async move {
            let mut schedule = tokio::time::interval(every);
            schedule.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            schedule.tick().await;
            loop {
                schedule.tick().await;
                if sender.send(Inbound::Check).is_err() {
                    return;
                }
            }
        })
    });
    Ok(())
}

/// When this process last did anything, and how many prompts it is running.
pub(super) struct Activity {
    in_flight: AtomicUsize,
    last: Mutex<Instant>,
}

/// One running prompt. Counted from before its worker starts until the
/// worker returns, so a prompt that has not reached its session yet is still
/// work in flight.
pub(super) struct InFlight(Arc<Activity>);

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.in_flight.fetch_sub(1, Ordering::SeqCst);
        self.0.touch();
    }
}

impl Activity {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            in_flight: AtomicUsize::new(0),
            last: Mutex::new(Instant::now()),
        })
    }

    pub(super) fn touch(&self) {
        if let Ok(mut last) = self.last.lock() {
            *last = Instant::now();
        }
    }

    pub(super) fn begin(self: &Arc<Self>) -> InFlight {
        self.in_flight.fetch_add(1, Ordering::SeqCst);
        self.touch();
        InFlight(self.clone())
    }

    fn quiet(&self) -> Duration {
        self.last
            .lock()
            .map(|last| last.elapsed())
            .unwrap_or_default()
    }
}

/// The `parked` event to send before ending, or none while there is work.
///
/// A session that cannot be read is reported on stderr and keeps the process
/// running: parking on a guess would end work this process could not see.
pub(super) fn park_notice(
    state: &ServerState,
    activity: &Activity,
    policy: &ParkPolicy,
) -> Option<Value> {
    match survey(state, activity, policy) {
        Ok(notice) => notice,
        Err(error) => {
            eprintln!("jeden rpc: not parking, a session could not be read: {error}");
            None
        }
    }
}

fn survey(
    state: &ServerState,
    activity: &Activity,
    policy: &ParkPolicy,
) -> Result<Option<Value>, String> {
    if activity.in_flight.load(Ordering::SeqCst) > 0 {
        return Ok(None);
    }
    let sessions: Vec<(String, AgentSession)> = state
        .sessions
        .lock()
        .map_err(|_| "sessions lock poisoned".to_string())?
        .iter()
        .map(|(id, session)| (id.clone(), session.clone()))
        .collect();
    let mut open = Vec::with_capacity(sessions.len());
    let mut blocked = 0;
    for (id, session) in &sessions {
        if !session.status()?.is_empty() {
            activity.touch();
            return Ok(None);
        }
        let blocker = session
            .completion()?
            .get("blocker")
            .cloned()
            .unwrap_or(Value::Null);
        if !blocker.is_null() {
            blocked += 1;
        }
        open.push(json!({"sessionId": id, "blocker": blocker}));
    }
    let quiet = activity.quiet();
    let reason = if quiet >= policy.after {
        "idle"
    } else if !sessions.is_empty() && blocked == sessions.len() && quiet >= policy.check_every {
        "blocked"
    } else {
        return Ok(None);
    };
    Ok(Some(json!({
        "type": "parked",
        "reason": reason,
        "quietSeconds": quiet.as_secs(),
        "parkAfterSeconds": policy.after.as_secs(),
        "sessions": open,
    })))
}
