#![cfg(unix)]
#[path = "../runtime/source/mod.rs"]
mod source;
use jeden::tool_runtime::runtime_ops::{pty, ArtifactSink, CancellationToken, OperationContext};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{fs, io::Write, path::{Path, PathBuf}, sync::mpsc, thread};
use std::os::unix::ffi::OsStrExt;

type Reply = mpsc::Receiver<Result<pty::PtyResult, String>>;
type Readiness = (PathBuf, String, mpsc::Sender<Result<(), String>>);
struct Journey {
    root: PathBuf,
    report: Value,
    scopes: Vec<PathBuf>,
    tokens: Vec<CancellationToken>,
    workers: Vec<thread::JoinHandle<()>>,
}
impl Journey {
    fn new() -> Result<Self, String> {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".build/terminal")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(root.join("artifacts")).map_err(|cause| format!("create terminal journey: {cause}"))?;
        Ok(Self { root, report: json!({"verdict":"failed", "commands":[], "calls":[], "surface":"native-PTY-API"}),
            scopes: Vec::new(), tokens: Vec::new(), workers: Vec::new() })
    }
    fn bind_source(&mut self) -> Result<(), String> {
        source::bind(&mut self.report)?;
        self.report["sourceSha256"] = json!(format!("{:x}", Sha256::digest(include_bytes!("lifecycle.rs"))));
        Ok(())
    }
    fn scope(&mut self, name: &str) -> Result<PathBuf, String> {
        let scope = self.root.join(name);
        fs::create_dir(&scope).map_err(|cause| format!("create isolated scope {}: {cause}", scope.display()))?;
        self.scopes.push(scope.clone());
        Ok(scope)
    }
    fn context(&self) -> OperationContext<'static> {
        OperationContext::new(CancellationToken::new(), ArtifactSink::new(self.root.join("artifacts")), self.root.clone())
    }
    fn launch(&mut self, scope: &Path, input: String, ready: Option<Readiness>) -> (CancellationToken, Reply) {
        let token = CancellationToken::new();
        self.tokens.push(token.clone());
        let worker_token = token.clone();
        let cwd = scope.to_path_buf();
        let root = self.root.clone();
        let (sender, receiver) = mpsc::channel();
        self.report["calls"].as_array_mut().expect("declared calls array").push(json!({"scope":scope,"input":input}));
        self.workers.push(thread::spawn(move || {
            // Construct the real context in its worker: progress sinks are thread-local.
            let context = OperationContext::new(worker_token, ArtifactSink::new(root.join("artifacts")), root);
            let context = if let Some((path, marker, sender)) = ready {
                let sent = std::cell::Cell::new(false);
                context.with_progress(std::sync::Arc::new(move |_| {
                    if sent.get() { return; }
                    let observation = match fs::read_to_string(&path) {
                        Ok(value) if value == marker => Some(Ok(())),
                        Ok(value) => Some(Err(format!("readiness {} is {value:?}, expected {marker:?}", path.display()))),
                        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => None,
                        Err(cause) => Some(Err(format!("read readiness {}: {cause}", path.display()))),
                    };
                    if let Some(observation) = observation {
                        sent.set(true);
                        if let Err(cause) = sender.send(observation) { eprintln!("terminal readiness receiver closed: {cause}"); }
                    }
                }))
            } else { context };
            let result = pty::execute(&context, &cwd, &cwd, &input, false);
            if let Err(cause) = sender.send(result) { eprintln!("terminal result receiver closed: {cause}"); }
        }));
        (token, receiver)
    }
    fn receive(&mut self, name: &str, reply: &Reply) -> Result<pty::PtyResult, String> {
        let result = reply.recv().map_err(|cause| format!("{name} result channel closed: {cause}"))?;
        let observation = match &result {
            Ok(value) => json!({"case":name,"ok":value.ok,"cancelled":value.cancelled,"code":value.code,
                "text":value.output.text,"session":value.session.session_id,"state":format!("{:?}",value.session.state),
                "cols":value.session.cols,"rows":value.session.rows}),
            Err(cause) => json!({"case":name,"error":cause}),
        };
        self.report["calls"].as_array_mut().ok_or("report.calls is not an array")?.push(observation);
        result
    }
    fn blocked(&mut self, scope: &Path, name: &str, suffix: &str) -> Result<(Reply, PathBuf), String> {
        let ready = scope.join(format!("{name}.ready"));
        let release = scope.join(format!("{name}.fifo"));
        let cpath = std::ffi::CString::new(release.as_os_str().as_bytes()).map_err(|cause| format!("FIFO path: {cause}"))?;
        if unsafe { libc::mkfifo(cpath.as_ptr(), libc::S_IRUSR | libc::S_IWUSR) } != libc::EXIT_SUCCESS {
            return Err(format!("create real FIFO {}: {}", release.display(), std::io::Error::last_os_error()));
        }
        let marker = uuid::Uuid::new_v4().to_string();
        let (sender, receiver) = mpsc::channel();
        let pending = ready.with_extension("pending");
        let command = format!("printf '%s' {marker} > {} && mv {} {} && printf '%s\\n' {marker} && IFS= read -r gate < {} && {{ {suffix}; }}", quote(&pending)?, quote(&pending)?, quote(&ready)?, quote(&release)?);
        let (_, reply) = self.launch(scope, command, Some((ready, marker, sender)));
        match receiver.recv() {
            Ok(observed) => observed?,
            Err(cause) => {
                let failure = match self.receive("blocked-startup", &reply) {
                    Err(error) => error,
                    Ok(value) => format!("command completed before readiness, ok={}", value.ok),
                };
                return Err(format!("readiness channel closed: {cause}; {failure}"));
            }
        }
        Ok((reply, release))
    }
    fn cleanup(&mut self) -> Result<(), String> {
        for token in &self.tokens { token.cancel(); }
        for scope in &self.scopes { pty::teardown_scope(scope); }
        let mut panics = Vec::new();
        for worker in self.workers.drain(..) {
            if let Err(cause) = worker.join() { panics.push(format!("{cause:?}")); }
        }
        if panics.is_empty() { Ok(()) } else { Err(format!("terminal workers panicked: {panics:?}")) }
    }
    fn retain(&mut self, result: &Result<(), String>) -> Result<(), String> {
        match result {
            Ok(()) => self.report["verdict"] = json!("passed"),
            Err(cause) => self.report["failure"] = json!(cause),
        }
        fs::write(self.root.join("report.json"), serde_json::to_vec_pretty(&self.report)
            .map_err(|cause| format!("serialize terminal report: {cause}"))?)
            .map_err(|cause| format!("retain terminal report: {cause}"))
    }
}
impl Drop for Journey {
    fn drop(&mut self) {
        if let Err(cause) = self.cleanup() { eprintln!("terminal journey cleanup failed: {cause}"); }
    }
}
fn quote(path: &Path) -> Result<String, String> {
    Ok(shell_words::quote(path.to_str().ok_or_else(|| format!("path {} is not UTF-8", path.display()))?).into_owned())
}
fn exercise(journey: &mut Journey) -> Result<(), String> {
    let a = journey.scope("first")?;
    let b = journey.scope("second")?;
    let marker = uuid::Uuid::new_v4().to_string();
    let variable = format!("jeden_terminal_{}", uuid::Uuid::new_v4().simple());
    let (_, warmup) = journey.launch(&a, format!("{variable}={marker}; export {variable}"), None);
    let original = journey.receive("initialize-first", &warmup)?;
    if !original.ok { return Err("first terminal could not establish persistent state".into()); }
    let retained = a.join("retained.txt");
    let suffix = format!("stty size; test \"${{{variable}}}\" = {marker} && printf '%s' {marker} > {}", quote(&retained)?);
    let (blocked, release) = journey.blocked(&a, "first-block", &suffix)?;
    let independent = b.join("independent.txt");
    let (_, other) = journey.launch(&b, format!("test -z \"${{{variable}+x}}\" && printf '%s' {marker} > {}", quote(&independent)?), None);
    if !journey.receive("independent-scope-while-first-blocked", &other)?.ok
        || fs::read_to_string(&independent).map_err(|cause| format!("read independent effect: {cause}"))? != marker {
        return Err("independent scope did not persist its isolated effect while the first was blocked".into());
    }
    match blocked.try_recv() {
        Err(mpsc::TryRecvError::Empty) => {}
        Err(cause) => return Err(format!("blocked terminal result channel failed: {cause}")),
        Ok(Err(cause)) => return Err(format!("first command refused before FIFO release: {cause}")),
        Ok(Ok(value)) => return Err(format!("first command finished without a FIFO writer: ok={}, cancelled={}, code={:?}, output={:?}", value.ok, value.cancelled, value.code, value.output.text)),
    }
    let resized = pty::resize(&journey.context(), &original.session.session_id, pty::MAX_PTY_COLS, pty::MIN_PTY_ROWS)
        .map_err(|cause| format!("resize while output pending: {cause}"))?;
    journey.report["resize"] = json!({"id":resized.session_id,"cols":resized.cols,"rows":resized.rows});
    // The documented minimum is the zero-width refusal boundary. Source: https://doc.rust-lang.org/std/primitive.u16.html#associatedconstant.MIN
    let zero_width = u16::MIN;
    match pty::resize(&journey.context(), &original.session.session_id, zero_width, resized.rows) {
        Err(pty::PtyError::InvalidDimensions { cols, rows }) if cols == zero_width && rows == resized.rows => {}
        other => return Err(format!("zero-width resize did not retain its invalid dimensions: {other:?}")),
    }
    let forbidden = a.join("cancelled-input.txt");
    let (cancel, queued) = journey.launch(&a, format!("printf '%s' {marker} > {}", quote(&forbidden)?), None);
    cancel.cancel();
    if journey.receive("cancel-contending-call-before-release", &queued).is_ok() {
        return Err("cancelled contending call was executed".into());
    }
    match fs::metadata(&forbidden) {
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {}
        Err(cause) => return Err(format!("inspect cancelled effect: {cause}")),
        Ok(_) => return Err("cancelled call wrote its input's side effect".into()),
    }
    fs::OpenOptions::new().write(true).open(&release).and_then(|mut pipe| pipe.write_all(b"release\n"))
        .map_err(|cause| format!("release actual FIFO reader: {cause}"))?;
    let completed = journey.receive("release-first-and-observe-native-size", &blocked)?;
    let dimensions = format!("{} {}", pty::MIN_PTY_ROWS, pty::MAX_PTY_COLS);
    if !completed.ok || completed.session.session_id != original.session.session_id
        || !completed.output.text.lines().any(|line| line.trim() == dimensions)
        || fs::read_to_string(&retained).map_err(|cause| format!("read persistent state effect: {cause}"))? != marker {
        return Err("released terminal lost its shell state, identity or native stty geometry".into());
    }
    let (active, _) = journey.blocked(&a, "teardown-block", "printf '%s' unexpectedly-released")?;
    pty::teardown_scope(&a);
    let cancelled = journey.receive("teardown-active-shell", &active)?;
    if !cancelled.cancelled || cancelled.session.state != pty::PtySessionState::Cancelled {
        return Err("scope teardown did not cancel the active shell".into());
    }
    let observed_ready = fs::read_to_string(a.join("teardown-block.ready"))
        .map_err(|cause| format!("read cancelled shell's persisted readiness: {cause}"))?;
    if !cancelled.output.text.lines().any(|line| line.trim() == observed_ready) {
        return Err("cancelled shell lost output it had already reported as received".into());
    }
    match pty::resize(&journey.context(), &original.session.session_id, resized.cols, resized.rows) {
        Err(pty::PtyError::SessionCancelled { session_id }) if session_id == original.session.session_id => {}
        other => return Err(format!("old terminal identity did not retain cancellation: {other:?}")),
    }
    let fresh_effect = a.join("fresh.txt");
    let (_, fresh) = journey.launch(&a, format!("test -z \"${{{variable}+x}}\" && printf '%s' {marker} > {}", quote(&fresh_effect)?), None);
    let fresh = journey.receive("fresh-generation-after-teardown", &fresh)?;
    if !fresh.ok || fresh.session.session_id == original.session.session_id
        || fs::read_to_string(&fresh_effect).map_err(|cause| format!("read fresh effect: {cause}"))? != marker {
        return Err("new scope generation reused cancelled identity or shell state".into());
    }
    Ok(())
}
#[test]
#[ignore = "requires an approved native target, clean committed candidate and enforced platform sandbox"]
fn real_terminal_scopes_resize_cancel_and_teardown() {
    let mut journey = Journey::new().expect("create retained real terminal journey");
    let mut result = journey.bind_source().and_then(|()| exercise(&mut journey));
    if let Err(cause) = journey.cleanup() {
        result = Err(match result { Ok(()) => cause, Err(original) => format!("{original}; cleanup: {cause}") });
    }
    journey.retain(&result).expect("retain native terminal observations");
    if let Err(cause) = result { panic!("{cause}; evidence: {}", journey.root.join("report.json").display()); }
}
