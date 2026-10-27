use super::super::platform::{ProcessSignal, ProcessTree};
use super::KernelLanguage;
use std::process::{Child, ExitStatus};
use std::sync::{Arc, Mutex, MutexGuard};

pub(super) type SharedControl = Arc<Mutex<KernelControl>>;

pub(super) struct KernelControl {
    pub(super) child: Child,
    tree: Box<dyn ProcessTree>,
    language: KernelLanguage,
    exited: Option<ExitStatus>,
    stopped: Option<bool>,
    stop_error: Option<String>,
    kill_complete: bool,
    pub(super) wake: Option<Box<dyn Fn() -> bool + Send>>,
}

impl KernelControl {
    pub(super) fn new(child: Child, tree: Box<dyn ProcessTree>, language: KernelLanguage) -> Self {
        Self { child, tree, language, exited: None, stopped: None, stop_error: None, wake: None, kill_complete: false }
    }
    pub(super) fn pid(&self) -> u32 { self.child.id() }
    pub(super) fn exit_status(&self) -> Option<ExitStatus> { self.exited }
    pub(super) fn cancelled(&self) -> bool { self.stopped == Some(true) }
    pub(super) fn require_live(&self) -> Result<(), String> {
        match self.stopped {
            None => Ok(()),
            Some(true) => Err(format!("{} kernel process {} was cancelled before evaluation", self.language.label(), self.pid())),
            Some(false) => Err(format!("{} kernel process {} was stopped before evaluation", self.language.label(), self.pid())),
        }
    }
    pub(super) fn alive(&mut self) -> Result<bool, String> {
        if self.exited.is_some() { return Ok(false); }
        match self.child.try_wait() {
            Ok(Some(status)) => { self.exited = Some(status); Ok(false) }
            Ok(None) => Ok(true),
            Err(error) => Err(format!("whether the {} kernel process {} still runs cannot be read: {error}", self.language.label(), self.pid())),
        }
    }
    fn reap(&mut self) -> Result<(), String> {
        if self.exited.is_some() { return Ok(()); }
        let process = self.pid();
        let language = self.language.label();
        let child = &mut self.child;
        let status = stado_wait::blocking(stado_wait::Kind::Process,
            format!("{language} kernel process {process} to end"), "this host", || child.wait())
            .map_err(|cause| format!("the {language} kernel process {process} could not be awaited: {cause}"))?;
        self.exited = Some(status);
        Ok(())
    }
    pub(super) fn interrupt(&mut self) -> Result<(), String> {
        if let Some(error) = &self.stop_error { return Err(error.clone()); }
        if self.stopped.is_some() { return Ok(()); }
        self.tree.signal(ProcessSignal::Interrupt).map_err(|cause| {
            format!("interrupt {} kernel process {}: {cause}", self.language.label(), self.pid())
        })
    }
    fn stop_and_reap(&mut self, cancelled: bool) -> Result<(), String> {
        if self.stopped.is_none() {
            self.stopped = Some(cancelled);
        }
        if !self.kill_complete {
            if let Err(cause) = self.tree.signal(ProcessSignal::Kill) {
                self.stop_error = Some(format!("{} kernel process {} kill failed: {cause}", self.language.label(), self.pid()));
            }
            self.kill_complete = true;
        }
        if let Some(wake) = &self.wake { wake(); }
        if let Some(error) = self.stop_error.clone() {
            let observed = match self.alive() {
                Ok(true) => "child has not exited".to_owned(),
                Ok(false) => {
                    let status = self.exited.ok_or_else(|| format!("{error}; child reported ended without an observed exit status"))?;
                    format!("child already exited ({status})")
                }
                Err(status) => format!("child status could not be read: {status}"),
            };
            return Err(format!("{error}; {observed}"));
        }
        self.reap()
    }
}

pub(super) fn lock(control: &SharedControl) -> Result<MutexGuard<'_, KernelControl>, String> {
    control.lock().map_err(|cause| format!("interpreter control lock poisoned: {cause}"))
}

pub(super) fn retire(shared: &SharedControl, cancelled: bool) {
    let mut control = match shared.lock() {
        Ok(control) => control,
        Err(poisoned) => {
            eprintln!("jeden: interpreter control lock poisoned during cleanup: {poisoned}; recovering its owned process to terminate it");
            poisoned.into_inner()
        }
    };
    if let Err(cause) = control.stop_and_reap(cancelled) { eprintln!("jeden: {cause}"); }
}

impl Drop for KernelControl {
    fn drop(&mut self) {
        // Covers partial launch and a spawn that lost its scope before registration.
        if !self.kill_complete {
            if let Err(cause) = self.stop_and_reap(false) { eprintln!("jeden: {cause}"); }
        }
    }
}
