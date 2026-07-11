use super::{BoundedOutput, OperationContext, OperationProgress, OutputCapture};
use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{LazyLock, Mutex, MutexGuard, TryLockError};
use std::thread;
use std::time::{Duration, Instant};

const POLL: Duration = Duration::from_millis(10);
pub const MIN_PTY_COLS: u16 = 1;
pub const MAX_PTY_COLS: u16 = 1_000;
pub const MIN_PTY_ROWS: u16 = 1;
pub const MAX_PTY_ROWS: u16 = 1_000;
const DEFAULT_PTY_COLS: u16 = 80;
const DEFAULT_PTY_ROWS: u16 = 24;

#[derive(Default)]
struct PtyRegistry {
    sessions: HashMap<PathBuf, PtyProcess>,
    terminal: HashMap<String, PtySessionState>,
    next_id: u64,
}

static SESSIONS: LazyLock<Mutex<PtyRegistry>> = LazyLock::new(|| Mutex::new(PtyRegistry::default()));

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PtySessionState { Live, Ended, Cancelled }

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PtySessionMetadata {
    pub session_id: String,
    pub cols: u16,
    pub rows: u16,
    pub state: PtySessionState,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PtyError {
    InvalidDimensions { cols: u16, rows: u16 },
    SessionNotFound { session_id: String },
    SessionEnded { session_id: String },
    SessionCancelled { session_id: String },
    OperationCancelled,
    System(String),
}

impl fmt::Display for PtyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDimensions { cols, rows } => write!(formatter, "invalid PTY dimensions {cols}x{rows}; cols and rows must be within {MIN_PTY_COLS}..={MAX_PTY_COLS} and {MIN_PTY_ROWS}..={MAX_PTY_ROWS}"),
            Self::SessionNotFound { session_id } => write!(formatter, "PTY session '{session_id}' does not exist"),
            Self::SessionEnded { session_id } => write!(formatter, "PTY session '{session_id}' has ended"),
            Self::SessionCancelled { session_id } => write!(formatter, "PTY session '{session_id}' was cancelled"),
            Self::OperationCancelled => formatter.write_str("PTY resize cancelled"),
            Self::System(message) => formatter.write_str(message),
        }
    }
}

impl Error for PtyError {}

pub struct PtyResult {
    pub ok: bool,
    pub timed_out: bool,
    pub cancelled: bool,
    pub reset: bool,
    pub code: Option<i32>,
    pub output: OutputCapture,
    pub session: PtySessionMetadata,
}

pub fn execute(
    context: &OperationContext<'_>, scope: &Path, cwd: &Path, input: &str,
    reset: bool, timeout: Duration,
) -> Result<PtyResult, String> {
    let key = scope.to_path_buf();
    let mut registry = SESSIONS.lock().map_err(|_| "PTY registry lock poisoned")?;
    if reset {
        if let Some(mut old) = registry.sessions.remove(&key) {
            let id = old.metadata.session_id.clone();
            old.terminate();
            registry.terminal.insert(id, PtySessionState::Cancelled);
        }
    }
    let mut session = if let Some(mut existing) = registry.sessions.remove(&key) {
        if existing.alive() {
            existing
        } else {
            let id = existing.metadata.session_id.clone();
            existing.terminate();
            registry.terminal.insert(id, PtySessionState::Ended);
            spawn_registered(&mut registry, cwd)?
        }
    } else {
        spawn_registered(&mut registry, cwd)?
    };
    let result = session.execute(context, input, reset, timeout);
    match result {
        Ok((result, healthy)) => {
            if healthy {
                registry.sessions.insert(key, session);
            } else {
                let id = session.metadata.session_id.clone();
                session.terminate();
                registry.terminal.insert(id, if result.cancelled { PtySessionState::Cancelled } else { PtySessionState::Ended });
            }
            Ok(result)
        }
        Err(error) => {
            let id = session.metadata.session_id.clone();
            session.terminate();
            registry.terminal.insert(id, PtySessionState::Ended);
            Err(error)
        }
    }
}

fn spawn_registered(registry: &mut PtyRegistry, cwd: &Path) -> Result<PtyProcess, String> {
    registry.next_id = registry.next_id.wrapping_add(1);
    PtyProcess::spawn(cwd, registry.next_id)
}
pub fn resize(
    context: &OperationContext<'_>, session_id: &str, cols: u16, rows: u16,
) -> Result<PtySessionMetadata, PtyError> {
    validate_dimensions(cols, rows)?;
    let mut registry = lock_registry_cancellable(context)?;
    let scope = registry.sessions.iter().find_map(|(scope, session)|
        (session.metadata.session_id == session_id).then(|| scope.clone()));
    let Some(scope) = scope else {
        return Err(match registry.terminal.get(session_id) {
            Some(PtySessionState::Ended) => PtyError::SessionEnded { session_id: session_id.into() },
            Some(PtySessionState::Cancelled) => PtyError::SessionCancelled { session_id: session_id.into() },
            _ => PtyError::SessionNotFound { session_id: session_id.into() },
        });
    };
    let alive = registry.sessions.get_mut(&scope).is_some_and(PtyProcess::alive);
    if !alive {
        if let Some(mut session) = registry.sessions.remove(&scope) { session.terminate(); }
        registry.terminal.insert(session_id.into(), PtySessionState::Ended);
        return Err(PtyError::SessionEnded { session_id: session_id.into() });
    }
    let session = registry.sessions.get_mut(&scope).expect("live PTY scope disappeared while registry was locked");
    session.resize(cols, rows)?;
    Ok(session.metadata.clone())
}

fn validate_dimensions(cols: u16, rows: u16) -> Result<(), PtyError> {
    if !(MIN_PTY_COLS..=MAX_PTY_COLS).contains(&cols) || !(MIN_PTY_ROWS..=MAX_PTY_ROWS).contains(&rows) {
        return Err(PtyError::InvalidDimensions { cols, rows });
    }
    Ok(())
}

fn lock_registry_cancellable(context: &OperationContext<'_>) -> Result<MutexGuard<'static, PtyRegistry>, PtyError> {
    loop {
        if context.cancellation().is_cancelled() { return Err(PtyError::OperationCancelled); }
        match SESSIONS.try_lock() {
            Ok(registry) => return Ok(registry),
            Err(TryLockError::WouldBlock) => thread::sleep(POLL),
            Err(TryLockError::Poisoned(_)) => return Err(PtyError::System("PTY registry lock poisoned".into())),
        }
    }
}
pub fn probe(cwd: &Path) -> Result<(), String> {
    let mut session = PtyProcess::spawn(cwd, 0)?;
    session.terminate();
    Ok(())
}


pub fn teardown_scope(scope: &Path) {
    if let Ok(mut registry) = SESSIONS.lock() {
        if let Some(mut session) = registry.sessions.remove(scope) {
            let id = session.metadata.session_id.clone();
            session.terminate();
            registry.terminal.insert(id, PtySessionState::Cancelled);
        }
    }
}

struct PtyProcess { child: Child, master: File, slave: File, group: u32, sequence: u64, metadata: PtySessionMetadata }

impl PtyProcess {
    fn spawn(cwd: &Path, id_sequence: u64) -> Result<Self, String> {
        let (master_fd, slave_fd) = open_pseudo_terminal()?;
        set_window_size(master_fd, DEFAULT_PTY_COLS, DEFAULT_PTY_ROWS).map_err(|error| error.to_string())?;
        set_window_size(slave_fd, DEFAULT_PTY_COLS, DEFAULT_PTY_ROWS).map_err(|error| error.to_string())?;
        let stdin_fd = duplicate(slave_fd)?;
        let stdout_fd = duplicate(slave_fd)?;
        let stderr_fd = duplicate(slave_fd)?;
        let mut command = Command::new("/bin/sh");
        command.arg("-i").current_dir(cwd);
        unsafe {
            command.stdin(Stdio::from_raw_fd(stdin_fd)).stdout(Stdio::from_raw_fd(stdout_fd)).stderr(Stdio::from_raw_fd(stderr_fd));
            command.pre_exec(|| { if setsid() < 0 { return Err(io::Error::last_os_error()); } Ok(()) });
        }
        let child = command.spawn().map_err(|error| format!("failed launching PTY shell: {error}"))?;
        let group = child.id();
        let mut master = unsafe { File::from_raw_fd(master_fd) };
        let slave = unsafe { File::from_raw_fd(slave_fd) };
        set_nonblocking(master_fd)?;
        master.write_all(b"stty -echo; printf '\n__JEDEN_PTY_READY__\n'\n").and_then(|_| master.flush()).map_err(|e|e.to_string())?;
        wait_for_bytes(&mut master, b"__JEDEN_PTY_READY__", Duration::from_secs(2))?;
        drain(&mut master);
        Ok(Self {
            child, master, slave, group, sequence: 0,
            metadata: PtySessionMetadata {
                session_id: format!("pty-{group}-{id_sequence}"),
                cols: DEFAULT_PTY_COLS,
                rows: DEFAULT_PTY_ROWS,
                state: PtySessionState::Live,
            },
        })
    }

    fn alive(&mut self) -> bool { self.child.try_wait().ok().flatten().is_none() }

    fn execute(&mut self, context: &OperationContext<'_>, input: &str, reset: bool, timeout: Duration) -> Result<(PtyResult,bool),String> {
        self.sequence = self.sequence.wrapping_add(1);
        let marker = format!("__JEDEN_PTY_{}_{}__", self.group, self.sequence);
        let command = format!("{input}\ns=$?; m='__JEDEN_PTY_'$$'_{}__'; printf '\\n%s:%s\\n' \"$m\" \"$s\"\n", self.sequence);
        self.master.write_all(command.as_bytes()).and_then(|_|self.master.flush()).map_err(|e|e.to_string())?;
        let mut output = BoundedOutput::new("pty", context.output_limits(), context.artifacts().clone());
        let deadline = context.effective_deadline(timeout);
        let mut pending = Vec::with_capacity(marker.len()+8192);
        let mut progress_total = 0u64;
        loop {
            if context.cancellation().is_cancelled() { self.signal(2); let mut session=self.metadata.clone();session.state=PtySessionState::Cancelled;return Ok((PtyResult{ok:false,timed_out:false,cancelled:true,reset,code:None,output:output.finish().map_err(|e|e.to_string())?,session},false)); }
            if Instant::now() >= deadline { self.signal(2); let mut session=self.metadata.clone();session.state=PtySessionState::Ended;return Ok((PtyResult{ok:false,timed_out:true,cancelled:false,reset,code:None,output:output.finish().map_err(|e|e.to_string())?,session},false)); }
            let mut chunk=[0u8;8192];
            match self.master.read(&mut chunk) {
                Ok(0) => { if !self.alive(){return Err("PTY shell exited before command completed".into());} }
                Ok(count) => {
                    pending.extend_from_slice(&chunk[..count]);
                    if let Some(position)=find_bytes(&pending,marker.as_bytes()) {
                        output.write_chunk(&pending[..position]).map_err(|e|e.to_string())?;
                        let suffix=&pending[position+marker.len()..];
                        let code=parse_marker_code(suffix);
                        let capture=output.finish().map_err(|e|e.to_string())?;
                        return Ok((PtyResult{ok:code==Some(0),timed_out:false,cancelled:false,reset,code,output:capture,session:self.metadata.clone()},true));
                    }
                    let retain=marker.len().saturating_add(16);
                    if pending.len()>retain { let emit=pending.len()-retain; output.write_chunk(&pending[..emit]).map_err(|e|e.to_string())?; pending.drain(..emit); }
                    progress_total = progress_total.saturating_add(count as u64);
                    context.progress(OperationProgress{stream:"pty",bytes:count as u64,total_bytes:progress_total});
                }
                Err(error) if error.kind()==io::ErrorKind::WouldBlock => thread::sleep(POLL),
                Err(error) => return Err(error.to_string()),
            }
        }
    }

    fn resize(&mut self, cols: u16, rows: u16) -> Result<(), PtyError> {
        set_window_size(self.master.as_raw_fd(), cols, rows)?;
        set_window_size(self.slave.as_raw_fd(), cols, rows)?;
        if unsafe { kill(-(self.group as i32), SIGWINCH) } < 0 {
            return Err(PtyError::System(format!("failed signalling PTY process group: {}", io::Error::last_os_error())));
        }
        self.metadata.cols = cols;
        self.metadata.rows = rows;
        Ok(())
    }
    fn signal(&self, signal:i32){unsafe{kill(-(self.group as i32),signal);}}
    fn terminate(&mut self){self.signal(15);let until=Instant::now()+Duration::from_millis(300);while Instant::now()<until{if self.child.try_wait().ok().flatten().is_some(){return;}thread::sleep(POLL);}self.signal(9);let _=self.child.wait();}
}

fn wait_for_bytes(file: &mut File, marker: &[u8], timeout: Duration) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    let mut pending = Vec::with_capacity(4096);
    while Instant::now() < deadline {
        let mut buffer = [0u8; 4096];
        match file.read(&mut buffer) {
            Ok(count) if count > 0 => {
                pending.extend_from_slice(&buffer[..count]);
                if find_bytes(&pending, marker).is_some() { return Ok(()); }
                if pending.len() > 8192 { pending.drain(..4096); }
            }
            Ok(_) => thread::sleep(POLL),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => thread::sleep(POLL),
            Err(error) => return Err(error.to_string()),
        }
    }
    Err("PTY shell did not complete startup handshake".into())
}
fn find_bytes(haystack:&[u8],needle:&[u8])->Option<usize>{haystack.windows(needle.len()).position(|window|window==needle)}
fn parse_marker_code(suffix:&[u8])->Option<i32>{let text=String::from_utf8_lossy(suffix);text.trim_start_matches(':').lines().next()?.trim().parse().ok()}
fn drain(file:&mut File){let mut buffer=[0u8;4096];loop{match file.read(&mut buffer){Ok(0)|Err(_)=>break,Ok(_)=>{}}}}
fn duplicate(fd:RawFd)->Result<RawFd,String>{let out=unsafe{dup(fd)};if out<0{Err(io::Error::last_os_error().to_string())}else{Ok(out)}}
fn set_nonblocking(fd:RawFd)->Result<(),String>{let flags=unsafe{fcntl(fd,3,0)};if flags<0{return Err(io::Error::last_os_error().to_string());}if unsafe{fcntl(fd,4,flags|4)}<0{return Err(io::Error::last_os_error().to_string());}Ok(())}
fn open_pseudo_terminal()->Result<(RawFd,RawFd),String>{let mut master=-1;let mut slave=-1;let rc=unsafe{openpty(&mut master,&mut slave,std::ptr::null_mut(),std::ptr::null(),std::ptr::null())};if rc<0{Err(io::Error::last_os_error().to_string())}else{Ok((master,slave))}}

#[repr(C)]
struct Winsize { rows: u16, cols: u16, xpixel: u16, ypixel: u16 }

fn set_window_size(fd: RawFd, cols: u16, rows: u16) -> Result<(), PtyError> {
    let size = Winsize { rows, cols, xpixel: 0, ypixel: 0 };
    if unsafe { ioctl(fd, TIOCSWINSZ, &size) } < 0 {
        return Err(PtyError::System(format!("failed resizing PTY: {}", io::Error::last_os_error())));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
const TIOCSWINSZ: u64 = 0x8008_7467;
#[cfg(target_os = "linux")]
const TIOCSWINSZ: u64 = 0x5414;
const SIGWINCH: i32 = 28;

#[cfg_attr(target_os="linux",link(name="util"))]
extern "C" { fn openpty(master:*mut i32,slave:*mut i32,name:*mut i8,termios:*const std::ffi::c_void,winsize:*const std::ffi::c_void)->i32; fn dup(fd:i32)->i32; fn setsid()->i32; fn kill(pid:i32,signal:i32)->i32; fn fcntl(fd:i32,cmd:i32,...)->i32; fn ioctl(fd:i32,request:u64,...)->i32; }

#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
mod tests {
    use super::super::{ArtifactSink, CancellationToken};
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(1);

    struct PtyFixture {
        root: PathBuf,
        scope: PathBuf,
    }

    impl PtyFixture {
        fn new(name: &str) -> Self {
            let unique = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "jeden-pty-{name}-{}-{unique}",
                std::process::id()
            ));
            fs::create_dir_all(&root).unwrap();
            let scope = root.join("scope");
            Self { root, scope }
        }

        fn context(&self) -> OperationContext<'static> {
            OperationContext::new(
                CancellationToken::new(),
                ArtifactSink::new(self.root.join("artifacts")),
            )
        }

        fn execute(&self, context: &OperationContext<'_>, input: &str) -> Result<PtyResult, String> {
            execute(
                context,
                &self.scope,
                &self.root,
                input,
                false,
                Duration::from_secs(2),
            )
        }
    }

    impl Drop for PtyFixture {
        fn drop(&mut self) {
            teardown_scope(&self.scope);
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn assert_shell_size(result: &PtyResult, rows: u16, cols: u16) {
        let expected = format!("{rows} {cols}");
        assert!(
            result.output.text.lines().any(|line| line.trim() == expected),
            "shell did not observe {rows} rows by {cols} columns; output was {:?}",
            result.output.text
        );
    }

    #[test]
    fn resize_updates_the_live_pty_and_repeated_resize_reaches_the_shell() {
        let fixture = PtyFixture::new("repeated-resize");
        let context = fixture.context();
        let created = fixture.execute(&context, "printf created").unwrap();
        let session_id = created.session.session_id;

        let first = resize(&context, &session_id, 111, 31).unwrap();
        assert_eq!((first.cols, first.rows), (111, 31));
        assert_shell_size(&fixture.execute(&context, "stty size").unwrap(), 31, 111);

        let second = resize(&context, &session_id, 73, 42).unwrap();
        assert_eq!((second.cols, second.rows), (73, 42));
        assert_shell_size(&fixture.execute(&context, "stty size").unwrap(), 42, 73);
    }

    #[test]
    fn resize_rejects_zero_and_oversize_dimensions() {
        let fixture = PtyFixture::new("invalid-dimensions");
        let context = fixture.context();
        let created = fixture.execute(&context, "printf created").unwrap();

        for (cols, rows) in [(0, 24), (80, 0), (MAX_PTY_COLS + 1, 24), (80, MAX_PTY_ROWS + 1)] {
            assert_eq!(
                resize(&context, &created.session.session_id, cols, rows),
                Err(PtyError::InvalidDimensions { cols, rows })
            );
        }
    }

    #[test]
    fn resize_rejects_missing_ended_cancelled_and_operation_cancelled_sessions() {
        let ended = PtyFixture::new("ended");
        let ended_context = ended.context();
        let missing_id = format!("never-issued-{}", std::process::id());
        assert_eq!(
            resize(&ended_context, &missing_id, 90, 30),
            Err(PtyError::SessionNotFound { session_id: missing_id })
        );

        let ended_id = ended.execute(&ended_context, "printf created").unwrap().session.session_id;
        assert!(ended.execute(&ended_context, "exit").is_err());
        assert_eq!(
            resize(&ended_context, &ended_id, 90, 30),
            Err(PtyError::SessionEnded { session_id: ended_id })
        );

        let torn_down = PtyFixture::new("torn-down");
        let teardown_context = torn_down.context();
        let teardown_id = torn_down.execute(&teardown_context, "printf created").unwrap().session.session_id;
        teardown_scope(&torn_down.scope);
        assert_eq!(
            resize(&teardown_context, &teardown_id, 90, 30),
            Err(PtyError::SessionCancelled { session_id: teardown_id })
        );

        let live = PtyFixture::new("cancelled-operation");
        let live_context = live.context();
        let live_id = live.execute(&live_context, "printf created").unwrap().session.session_id;
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let cancelled_context = OperationContext::new(
            cancellation,
            ArtifactSink::new(live.root.join("cancelled-artifacts")),
        );
        assert_eq!(
            resize(&cancelled_context, &live_id, 90, 30),
            Err(PtyError::OperationCancelled)
        );
    }
}
