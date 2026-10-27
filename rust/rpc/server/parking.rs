//! Parking a `jeden rpc` that has nothing to do.
//!
//! Stado places every Jeden Desktop tab as `stado workload attach
//! jeden-session`, and the attach holds the kind's declared reservation for
//! exactly as long as this process lives. A handful of idle tabs — resumed
//! sessions stopped at their first model call with a completion blocker, or
//! an empty tab — can hold more cores than a laptop has while using no CPU, and the
//! laptop then refuses every fleet build with `reservations_exhausted`.
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
//! The check is a periodic schedule at the declared park time that looks at
//! the process; it never interrupts anything. A turn that runs for an hour is
//! work in flight for that hour, and nothing here ends it.

use super::operations::wire::read_frame;
use super::ServerState;
use crate::sdk::AgentSession;
use serde_json::{json, Value};
use std::io::BufRead;
use std::sync::mpsc::{self, Receiver, Sender};
use crate::panicked::held;
use std::sync::{Arc, Mutex, Weak};
use std::thread;
use std::time::{Duration, Instant};

/// The setting Stado's workload declaration fills from the kind's
/// `park_after_seconds`.
const PARK_AFTER_SETTING: &str = "JEDEN_PARK_AFTER_SECONDS";

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
            .parse::<std::num::NonZeroU64>()
            .map_err(|error| {
                format!("{PARK_AFTER_SETTING} must be a whole number of seconds above zero, got {raw:?}: {error}")
            })?
            .get();
        Ok(Some(Self {
            after: Duration::from_secs(seconds),
            check_every: Duration::from_secs(seconds),
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
pub(super) fn inbox<R>(
    mut input: R,
    policy: Option<&ParkPolicy>,
) -> Result<Receiver<Inbound>, String>
where
    R: BufRead + Send + 'static,
{
    let (sender, receiver) = mpsc::channel();
    let frames = sender.clone();
    thread::spawn(move || loop {
        let inbound = match read_frame(&mut input) {
            Ok(Some(frame)) => Inbound::Frame(Ok(frame)),
            Ok(None) => {
                // The reader ends here either way; a closed receiver has
                // already stopped listening for the end.
                let _unreceived = frames.send(Inbound::Closed);
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

/// When this process last did anything, and the prompts it is running.
///
/// A request that panics while holding either lock poisons it; what the lock
/// guards is still true, so it is read and written through the poison. Read
/// as nothing instead, one panic made the process look busy or quiet for the
/// rest of its life and it never parked again.
pub(super) struct Activity {
    running: Mutex<Vec<Weak<()>>>,
    last: Mutex<Instant>,
}

/// One running prompt. Held from before its worker starts until the worker
/// returns, so a prompt that has not reached its session yet is still work in
/// flight; it stops counting when it is dropped.
pub(super) struct InFlight {
    activity: Arc<Activity>,
    _prompt: Arc<()>,
}

impl Drop for InFlight {
    fn drop(&mut self) {
        self.activity.touch();
    }
}

impl Activity {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            running: Mutex::new(Vec::new()),
            last: Mutex::new(Instant::now()),
        })
    }

    pub(super) fn touch(&self) {
        *held(&self.last) = Instant::now();
    }

    pub(super) fn begin(self: &Arc<Self>) -> InFlight {
        let prompt = Arc::new(());
        held(&self.running).push(Arc::downgrade(&prompt));
        self.touch();
        InFlight {
            activity: self.clone(),
            _prompt: prompt,
        }
    }

    /// Whether any prompt is still running; ended prompts are forgotten.
    fn working(&self) -> bool {
        let mut running = held(&self.running);
        running.retain(|prompt| prompt.upgrade().is_some());
        !running.is_empty()
    }

    fn quiet(&self) -> Duration {
        held(&self.last).elapsed()
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
    if activity.working() {
        return Ok(None);
    }
    let sessions: Vec<(String, AgentSession)> = crate::panicked::held(&state.sessions)
        .iter()
        .map(|(id, session)| (id.clone(), session.clone()))
        .collect();
    let mut open = Vec::with_capacity(sessions.len());
    // Whether every open session carries a completion blocker.
    let mut every_blocked = !sessions.is_empty();
    for (id, session) in &sessions {
        if !session.status()?.is_empty() {
            activity.touch();
            return Ok(None);
        }
        // A completion without a blocker field has none.
        let blocker = match session.completion()?.get("blocker") {
            Some(blocker) => blocker.clone(),
            None => Value::Null,
        };
        if blocker.is_null() {
            every_blocked = false;
        }
        open.push(json!({"sessionId": id, "blocker": blocker}));
    }
    let quiet = activity.quiet();
    let reason = if quiet >= policy.after {
        "idle"
    } else if every_blocked && quiet >= policy.check_every {
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
