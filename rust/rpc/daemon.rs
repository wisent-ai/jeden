mod connection;
mod dispatch;
use connection::reject_overloaded;
use dispatch::*;

use super::service::{ServiceError, SessionBackend, SessionService, SubmitOutcome};
use super::tenant::{TenantDirectory, TenantError};
use super::tls::ReloadableTlsAcceptor;
use super::transport::{AuthenticatedConnection, ErrorV1, ReconnectTokens, RequestEnvelopeV1};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{watch, Semaphore};
use tokio::task::JoinSet;

use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

type Job = Box<dyn FnOnce() + Send + 'static>;
const STARTING: u8 = 0;
const READY: u8 = 1;
const DRAINING: u8 = 2;
const STOPPED: u8 = 3;

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
    Backpressure { retry_after_millis: u64 },
}

struct ExecutorInner {
    state: AtomicU8,
    queued_and_running: AtomicUsize,
    sender: Mutex<Option<mpsc::SyncSender<Job>>>,
    idle: Condvar,
    idle_lock: Mutex<()>,
}

pub struct BoundedExecutor {
    inner: Arc<ExecutorInner>,
    workers: Mutex<Vec<JoinHandle<()>>>,
}

impl BoundedExecutor {
    pub fn new(worker_count: usize, queue_capacity: usize) -> Result<Self, String> {
        if worker_count == 0 || queue_capacity == 0 {
            return Err("worker_count and queue_capacity must be non-zero".into());
        }
        let (sender, receiver) = mpsc::sync_channel::<Job>(queue_capacity);
        let receiver = Arc::new(Mutex::new(receiver));
        let inner = Arc::new(ExecutorInner {
            state: AtomicU8::new(STARTING),
            queued_and_running: AtomicUsize::new(0),
            sender: Mutex::new(Some(sender)),
            idle: Condvar::new(),
            idle_lock: Mutex::new(()),
        });
        let mut workers = Vec::with_capacity(worker_count);
        for index in 0..worker_count {
            let receiver = receiver.clone();
            let worker_inner = inner.clone();
            workers.push(
                thread::Builder::new()
                    .name(format!("jeden-session-{}", index))
                    .spawn(move || loop {
                        let job = match receiver.lock() {
                            Ok(receiver) => receiver.recv(),
                            Err(_) => break,
                        };
                        match job {
                            Ok(job) => {
                                job();
                                if worker_inner
                                    .queued_and_running
                                    .fetch_sub(1, Ordering::AcqRel)
                                    == 1
                                {
                                    worker_inner.idle.notify_all();
                                }
                            }
                            Err(_) => break,
                        }
                    })
                    .map_err(|error| error.to_string())?,
            );
        }
        inner.state.store(READY, Ordering::Release);
        Ok(Self {
            inner,
            workers: Mutex::new(workers),
        })
    }

    pub fn readiness(&self) -> Readiness {
        match self.inner.state.load(Ordering::Acquire) {
            STARTING => Readiness::Starting,
            READY => Readiness::Ready,
            DRAINING => Readiness::Draining,
            _ => Readiness::Stopped,
        }
    }

    pub fn submit(&self, job: impl FnOnce() + Send + 'static) -> Result<(), SubmitError> {
        if self.inner.state.load(Ordering::Acquire) != READY {
            return Err(SubmitError::NotReady);
        }
        self.inner.queued_and_running.fetch_add(1, Ordering::AcqRel);
        let result = self
            .inner
            .sender
            .lock()
            .ok()
            .and_then(|sender| sender.as_ref().cloned())
            .ok_or(SubmitError::NotReady)
            .and_then(|sender| {
                sender.try_send(Box::new(job)).map_err(|error| match error {
                    mpsc::TrySendError::Full(_) => SubmitError::Backpressure {
                        retry_after_millis: 100,
                    },
                    mpsc::TrySendError::Disconnected(_) => SubmitError::NotReady,
                })
            });
        if result.is_err() {
            self.inner.queued_and_running.fetch_sub(1, Ordering::AcqRel);
        }
        result
    }

    pub fn drain(&self, timeout: Duration) -> Result<(), String> {
        let prior =
            self.inner
                .state
                .compare_exchange(READY, DRAINING, Ordering::AcqRel, Ordering::Acquire);
        if prior.is_err() && self.inner.state.load(Ordering::Acquire) != DRAINING {
            return Ok(());
        }
        let deadline = Instant::now() + timeout;
        let mut guard = self
            .inner
            .idle_lock
            .lock()
            .map_err(|_| "executor idle lock poisoned")?;
        while self.inner.queued_and_running.load(Ordering::Acquire) != 0 {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err("graceful drain timed out with active operations".into());
            }
            let waited = self
                .inner
                .idle
                .wait_timeout(guard, remaining)
                .map_err(|_| "executor idle lock poisoned")?;
            guard = waited.0;
        }
        self.inner
            .sender
            .lock()
            .map_err(|_| "executor sender lock poisoned")?
            .take();
        let workers = std::mem::take(
            &mut *self
                .workers
                .lock()
                .map_err(|_| "executor workers lock poisoned")?,
        );
        for worker in workers {
            worker.join().map_err(|_| "executor worker panicked")?;
        }
        self.inner.state.store(STOPPED, Ordering::Release);
        Ok(())
    }
}

impl Drop for BoundedExecutor {
    fn drop(&mut self) {
        self.inner.state.store(DRAINING, Ordering::Release);
        if let Ok(mut sender) = self.inner.sender.lock() {
            sender.take();
        }
        if let Ok(workers) = self.workers.get_mut() {
            for worker in workers.drain(..) {
                let _ = worker.join();
            }
        }
        self.inner.state.store(STOPPED, Ordering::Release);
    }
}

#[derive(Debug, Clone)]
pub struct HeadlessConfig {
    pub max_frame_bytes: usize,
    pub read_timeout: Duration,
    pub write_timeout: Duration,
    pub drain_timeout: Duration,
    pub max_connections: usize,
    pub reconnect_key: Vec<u8>,
    pub reconnect_ttl: Duration,
}

impl Default for HeadlessConfig {
    fn default() -> Self {
        Self {
            max_frame_bytes: 1024 * 1024,
            read_timeout: Duration::from_secs(30),
            write_timeout: Duration::from_secs(10),
            drain_timeout: Duration::from_secs(30),
            max_connections: 128,
            reconnect_key: vec![0; 32],
            reconnect_ttl: Duration::from_secs(300),
        }
    }
}

pub struct HeadlessDaemon<B: SessionBackend> {
    tls: ReloadableTlsAcceptor,
    directory: TenantDirectory,
    service: Arc<SessionService<B>>,
    config: HeadlessConfig,
    reconnect: ReconnectTokens,
}

impl<B: SessionBackend> HeadlessDaemon<B> {
    pub fn new(
        tls: ReloadableTlsAcceptor,
        directory: TenantDirectory,
        service: Arc<SessionService<B>>,
        config: HeadlessConfig,
    ) -> Result<Self, String> {
        if config.max_frame_bytes == 0 || config.max_connections == 0 {
            return Err("max_frame_bytes and max_connections must be non-zero".into());
        }
        let reconnect = ReconnectTokens::new(config.reconnect_key.clone())
            .map_err(|_| "reconnect key must contain at least 32 bytes")?;
        Ok(Self {
            tls,
            directory,
            service,
            config,
            reconnect,
        })
    }

    pub fn local_addr(listener: &TcpListener) -> Result<SocketAddr, String> {
        listener.local_addr().map_err(|error| error.to_string())
    }

    pub async fn serve(
        self: Arc<Self>,
        listener: TcpListener,
        mut shutdown: watch::Receiver<bool>,
    ) -> Result<(), String> {
        let admission = Arc::new(Semaphore::new(self.config.max_connections));
        let mut connections = JoinSet::new();
        loop {
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() { break; }
                }
                accepted = listener.accept() => {
                    let (stream, _) = accepted.map_err(|error| format!("TCP accept failed: {error}"))?;
                    let daemon = self.clone();
                    let permit = admission.clone().try_acquire_owned();
                    connections.spawn(async move {
                        let _permit = match permit {
                            Ok(permit) => permit,
                            Err(_) => { reject_overloaded(stream, daemon.config.write_timeout).await; return; }
                        };
                        let _ = daemon.serve_connection(stream).await;
                    });
                }
            }
        }
        let service = self.service.clone();
        let drain_timeout = self.config.drain_timeout;
        tokio::task::spawn_blocking(move || service.drain(drain_timeout))
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| format!("graceful drain failed: {error:?}"))?;
        while connections.join_next().await.is_some() {}
        Ok(())
    }
}
