use super::super::super::platform::{ProcessSignal, PtySession};
use super::{PtyError, PtySessionMetadata, PtySessionState};
use std::sync::{Arc, Mutex, MutexGuard};

pub(super) type SharedControl = Arc<Mutex<PtyControl>>;

/// Native control operations never hold the command's blocking output reader.
pub(super) struct PtyControl {
    pub(super) session: Box<dyn PtySession>,
    pub(super) metadata: PtySessionMetadata,
    pub(super) wake: Box<dyn Fn() -> bool + Send>,
    pub(super) kill_complete: bool,
}

pub(super) fn lock(control: &SharedControl) -> Result<MutexGuard<'_, PtyControl>, String> {
    control.lock().map_err(|_| "PTY control lock poisoned".into())
}

impl PtyControl {
    pub(super) fn require_live(&self) -> Result<(), PtyError> {
        match self.metadata.state {
            PtySessionState::Live => Ok(()),
            PtySessionState::Ended => Err(PtyError::SessionEnded {
                session_id: self.metadata.session_id.clone(),
            }),
            PtySessionState::Cancelled => Err(PtyError::SessionCancelled {
                session_id: self.metadata.session_id.clone(),
            }),
        }
    }

    pub(super) fn alive(&mut self) -> Result<bool, String> {
        if self.metadata.state != PtySessionState::Live {
            return Ok(false);
        }
        self.session.alive().map_err(|error| {
            format!("PTY session '{}' status could not be read: {error}", self.metadata.session_id)
        })
    }

    pub(super) fn resize(&mut self, cols: u16, rows: u16) -> Result<(), PtyError> {
        self.require_live()?;
        if !self.alive().map_err(PtyError::System)? {
            return Err(PtyError::SessionEnded {
                session_id: self.metadata.session_id.clone(),
            });
        }
        self.session.resize(cols, rows).map_err(|error| {
            PtyError::System(format!(
                "PTY session '{}' resize to {cols}x{rows} failed: {error}",
                self.metadata.session_id
            ))
        })?;
        self.metadata.cols = cols;
        self.metadata.rows = rows;
        Ok(())
    }

    /// Preserve the first terminal transition, including a concurrent teardown.
    pub(super) fn terminate(&mut self, state: PtySessionState) -> PtySessionState {
        if self.metadata.state == PtySessionState::Live {
            self.metadata.state = state;
        }
        // A panic during signaling did not establish that the kill completed.
        if !self.kill_complete {
            let result = self.session.signal(ProcessSignal::Kill);
            self.kill_complete = true;
            if let Err(error) = result {
                let observed = match self.session.alive() {
                    Ok(true) => "child has not exited".to_owned(),
                    Ok(false) => "child already exited".to_owned(),
                    Err(status) => format!("child status could not be read: {status}"),
                };
                eprintln!(
                    "jeden: PTY session '{}' kill failed: {error}; {observed}",
                    self.metadata.session_id
                );
            }
        }
        (self.wake)();
        self.metadata.state
    }
}
