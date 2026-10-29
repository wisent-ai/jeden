//! The bounded pool of threads a headless host runs session work on. Work is
//! refused while the pool is not ready or its queue is full, and a drain
//! waits for the last operation before the pool stops: a shutdown that
//! abandons an operation mid-write is the failure this avoids.

use std::num::NonZeroUsize;
use std::sync::{mpsc, Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};

type Job = Box<dyn FnOnce() + Send + 'static>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readiness {
    Starting,
    Ready,
    Draining,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmitError {
    NotReady,
    Backpressure,
}

/// The value behind `mutex`, also when a panicking holder poisoned it: every
/// value kept here is replaced whole, never left half-written.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

struct ExecutorInner {
    state: Mutex<Readiness>,
    /// One entry per job queued or running; the pool is idle when it is
    /// empty. A list of units counts without a counter written by hand.
    pending: Mutex<Vec<()>>,
    sender: Mutex<Option<mpsc::SyncSender<Job>>>,
    idle: Condvar,
}

pub struct BoundedExecutor {
    inner: Arc<ExecutorInner>,
    workers: Mutex<Vec<JoinHandle<()>>>,
}

impl BoundedExecutor {
    /// `workers` threads taking jobs from a queue of `queue` places, both
    /// named by the host's declaration.
    pub fn new(workers: NonZeroUsize, queue: NonZeroUsize) -> Result<Self, String> {
        let (sender, receiver) = mpsc::sync_channel::<Job>(queue.get());
        let receiver = Arc::new(Mutex::new(receiver));
        let inner = Arc::new(ExecutorInner {
            state: Mutex::new(Readiness::Starting),
            pending: Mutex::new(Vec::new()),
            sender: Mutex::new(Some(sender)),
            idle: Condvar::new(),
        });
        let mut spawned = Vec::with_capacity(workers.get());
        for (index, ()) in std::iter::repeat_n((), workers.get()).enumerate() {
            let receiver = receiver.clone();
            let worker = inner.clone();
            let handle = thread::Builder::new()
                .name(format!("jeden-session-{index}"))
                .spawn(move || loop {
                    let job = match receiver.lock() {
                        Ok(receiver) => receiver.recv(),
                        Err(poisoned) => {
                            eprintln!("jeden daemon: session worker {index} stops: the job queue is poisoned: {poisoned}");
                            break;
                        }
                    };
                    // Every sender is gone: the pool is draining or dropped.
                    let Ok(job) = job else { break };
                    job();
                    let mut pending = lock(&worker.pending);
                    pending.pop();
                    if pending.is_empty() {
                        worker.idle.notify_all();
                    }
                })
                .map_err(|error| format!("session worker {index} could not be started: {error}"))?;
            spawned.push(handle);
        }
        *lock(&inner.state) = Readiness::Ready;
        Ok(Self {
            inner,
            workers: Mutex::new(spawned),
        })
    }

    pub fn readiness(&self) -> Readiness {
        *lock(&self.inner.state)
    }

    pub fn submit(&self, job: impl FnOnce() + Send + 'static) -> Result<(), SubmitError> {
        if self.readiness() != Readiness::Ready {
            return Err(SubmitError::NotReady);
        }
        let sender = lock(&self.inner.sender)
            .clone()
            .ok_or(SubmitError::NotReady)?;
        let mut pending = lock(&self.inner.pending);
        pending.push(());
        match sender.try_send(Box::new(job)) {
            Ok(()) => Ok(()),
            Err(error) => {
                pending.pop();
                Err(match error {
                    mpsc::TrySendError::Full(_) => SubmitError::Backpressure,
                    mpsc::TrySendError::Disconnected(_) => SubmitError::NotReady,
                })
            }
        }
    }

    /// Stop accepting work and wait for what is already running; the wait
    /// ends when the last operation ends.
    pub fn drain(&self) -> Result<(), String> {
        {
            let mut state = lock(&self.inner.state);
            match *state {
                Readiness::Ready => *state = Readiness::Draining,
                Readiness::Draining => {}
                Readiness::Starting | Readiness::Stopped => return Ok(()),
            }
        }
        let mut pending = lock(&self.inner.pending);
        while !pending.is_empty() {
            pending = match self.inner.idle.wait(pending) {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
        }
        drop(pending);
        lock(&self.inner.sender).take();
        let workers = std::mem::take(&mut *lock(&self.workers));
        for worker in workers {
            worker
                .join()
                .map_err(|panic| crate::panicked::refusal("an executor worker", &*panic))?;
        }
        *lock(&self.inner.state) = Readiness::Stopped;
        Ok(())
    }
}

impl Drop for BoundedExecutor {
    fn drop(&mut self) {
        *lock(&self.inner.state) = Readiness::Draining;
        // Taking the sender ends every worker's wait, so the joins return.
        lock(&self.inner.sender).take();
        let workers = std::mem::take(&mut *lock(&self.workers));
        for worker in workers {
            // drain() refuses a panicked worker; a drop has no caller to
            // refuse to, so the panic is said instead of lost.
            if let Err(panic) = worker.join() {
                eprintln!(
                    "jeden daemon: an executor worker panicked before shutdown: {}",
                    crate::panicked::message(&*panic)
                );
            }
        }
        *lock(&self.inner.state) = Readiness::Stopped;
    }
}
