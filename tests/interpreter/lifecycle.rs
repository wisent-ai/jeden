#![cfg(unix)]
#[path = "../runtime/source/mod.rs"]
mod source;
use jeden::tool_runtime::runtime_ops::{kernel::{self, KernelLanguage, KernelResult}, ArtifactSink, CancellationToken, OperationContext};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{fs, path::{Path, PathBuf}, sync::mpsc, thread};

type Reply = mpsc::Receiver<Result<KernelResult, String>>;
fn label(language: KernelLanguage) -> &'static str {
    match language { KernelLanguage::Python => "python", KernelLanguage::JavaScript => "javascript" }
}
struct Journey {
    root: PathBuf,
    report: Value,
    scopes: Vec<PathBuf>,
    tokens: Vec<CancellationToken>,
    workers: Vec<thread::JoinHandle<()>>,
}
impl Journey {
    fn new(language: KernelLanguage) -> Result<Self, String> {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".build/interpreter")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(root.join("artifacts")).map_err(|cause| format!("create interpreter journey: {cause}"))?;
        Ok(Self { root, report: json!({"verdict":"failed","surface":"native-kernel-API","language":label(language),"commands":[],"calls":[]}),
            scopes: Vec::new(), tokens: Vec::new(), workers: Vec::new() })
    }
    fn scope(&mut self, name: &str) -> Result<PathBuf, String> {
        let path = self.root.join(name);
        fs::create_dir(&path).map_err(|cause| format!("create isolated interpreter scope: {cause}"))?;
        self.scopes.push(path.clone());
        Ok(path)
    }
    fn launch(&mut self, scope: &Path, language: KernelLanguage, code: String, ready: Option<mpsc::Sender<()>>) -> (CancellationToken, Reply) {
        let token = CancellationToken::new();
        self.tokens.push(token.clone());
        let worker_token = token.clone();
        let root = self.root.clone();
        let scope = scope.to_path_buf();
        self.report["calls"].as_array_mut().expect("declared calls array")
            .push(json!({"scope":scope,"language":label(language),"code":code}));
        let (sender, receiver) = mpsc::channel();
        self.workers.push(thread::spawn(move || {
            let context = OperationContext::new(worker_token, ArtifactSink::new(root.join("artifacts")), root);
            let context = if let Some(sender) = ready {
                let sent = std::cell::Cell::new(false);
                context.with_progress(std::sync::Arc::new(move |_| {
                    if !sent.replace(true) {
                        if let Err(cause) = sender.send(()) { eprintln!("interpreter readiness receiver closed: {cause}"); }
                    }
                }))
            } else { context };
            let result = kernel::evaluate(&context, &scope, &scope, language, &code, false);
            if let Err(cause) = sender.send(result) { eprintln!("interpreter result receiver closed: {cause}"); }
        }));
        (token, receiver)
    }
    fn receive(&mut self, name: &str, reply: &Reply) -> Result<KernelResult, String> {
        let result = reply.recv().map_err(|cause| format!("{name} result channel closed: {cause}"))?;
        let observation = match &result {
            Ok(value) => json!({"case":name,"ok":value.ok,"cancelled":value.cancelled,"error":value.error,
                "stdout":value.stdout.text,"stderr":value.stderr.text,"display":value.display.text}),
            Err(cause) => json!({"case":name,"error":cause}),
        };
        self.report["calls"].as_array_mut().ok_or("report.calls is not an array")?.push(observation);
        result
    }
    fn query(&mut self, scope: &Path, language: KernelLanguage, variable: &str, name: &str) -> Result<bool, String> {
        let key = json!(variable).to_string();
        let code = match language {
            KernelLanguage::Python => format!("import json; print(json.dumps({key} in globals()))"),
            KernelLanguage::JavaScript => format!("console.log(JSON.stringify(Object.prototype.hasOwnProperty.call(globalThis,{key})))"),
        };
        let (_, reply) = self.launch(scope, language, code, None);
        let result = self.receive(name, &reply)?;
        if !result.ok { return Err(format!("{name} evaluation refused: {:?}; {}", result.error, result.stderr.text)); }
        serde_json::from_str(result.stdout.text.trim()).map_err(|cause| format!("{name} did not produce its state boolean: {cause}; {}", result.stdout.text))
    }
    fn cleanup(&mut self) -> Result<(), String> {
        for token in &self.tokens { token.cancel(); }
        for scope in &self.scopes { kernel::teardown_scope(scope); }
        let mut panics = Vec::new();
        for worker in self.workers.drain(..) {
            if let Err(cause) = worker.join() { panics.push(format!("{cause:?}")); }
        }
        if panics.is_empty() { Ok(()) } else { Err(format!("interpreter workers panicked: {panics:?}")) }
    }
}
impl Drop for Journey {
    fn drop(&mut self) {
        if let Err(cause) = self.cleanup() { eprintln!("interpreter journey cleanup failed: {cause}"); }
    }
}
fn assert_absent(journey: &mut Journey, pid_file: &Path, observation: &str) -> Result<(), String> {
    let text = fs::read_to_string(pid_file).map_err(|cause| format!("read actual interpreter PID: {cause}"))?;
    let pid = text.parse::<libc::pid_t>().map_err(|cause| format!("invalid actual interpreter PID {text:?}: {cause}"))?;
    if !pid.is_positive() { return Err(format!("actual interpreter PID is not positive: {pid}")); }
    // POSIX null signal checks process existence without sending a signal. Source: https://pubs.opengroup.org/onlinepubs/9799919799/functions/kill.html
    let null_signal = 0;
    let status = unsafe { libc::kill(pid, null_signal) };
    if status == libc::EXIT_SUCCESS {
        journey.report[observation] = json!({"pid":pid,"probe":status,"exists":true});
        return Err(format!("interpreter process {pid} still exists after cancellation"));
    }
    let cause = std::io::Error::last_os_error();
    journey.report[observation] = json!({"pid":pid,"probe":status,"errno":cause.raw_os_error()});
    if cause.raw_os_error() != Some(libc::ESRCH) {
        return Err(format!("interpreter process {pid} absence probe refused: status={status}, cause={cause}"));
    }
    Ok(())
}
fn exercise(journey: &mut Journey, language: KernelLanguage) -> Result<(), String> {
    let a = journey.scope("first")?;
    let b = journey.scope("second")?;
    let variable = format!("jeden_interpreter_{}", uuid::Uuid::new_v4().simple());
    let marker = uuid::Uuid::new_v4().to_string();
    let key = json!(&variable).to_string();
    let value = json!(&marker).to_string();
    let pid_file = a.join("interpreter.pid");
    let code = match language {
        KernelLanguage::Python => format!("globals()[{key}]={value}; __import__('pathlib').Path({}).write_text(str(__import__('os').getpid()))", json!(pid_file.to_str().ok_or("PID path is not UTF-8")?)),
        KernelLanguage::JavaScript => format!("globalThis[{key}]={value}; undefined"),
    };
    let (_, warmup) = journey.launch(&a, language, code, None);
    if !journey.receive("initialize-persistent-state", &warmup)?.ok { return Err("interpreter state initialization failed".into()); }
    if !journey.query(&a, language, &variable, "persistent-state-before-blocking")? {
        return Err("a later evaluation did not retain the declared variable".into());
    }
    let code = match language {
        KernelLanguage::Python => format!("print({value},flush=True)\nwhile True:\n pass"),
        KernelLanguage::JavaScript => format!("console.log({value}); while(true){{}}"),
    };
    let (sender, ready) = mpsc::channel();
    let (_, active) = journey.launch(&a, language, code, Some(sender));
    if let Err(cause) = ready.recv() {
        let actual = match journey.receive("blocked-startup", &active) {
            Err(error) => error,
            Ok(value) => format!("evaluation finished before readiness: ok={}, error={:?}", value.ok, value.error),
        };
        return Err(format!("readiness channel closed: {cause}; {actual}"));
    }
    if journey.query(&b, language, &variable, "independent-scope-while-first-blocked")? {
        return Err("another scope inherited the first scope's variable".into());
    }
    let other_variable = format!("{variable}_independent");
    let other_key = json!(&other_variable).to_string();
    let code = match language {
        KernelLanguage::Python => format!("globals()[{other_key}]={value}"),
        KernelLanguage::JavaScript => format!("globalThis[{other_key}]={value}; undefined"),
    };
    let (_, other_state) = journey.launch(&b, language, code, None);
    if !journey.receive("persist-independent-scope-state", &other_state)?.ok {
        return Err("independent scope did not persist its own state".into());
    }
    let other = match language { KernelLanguage::Python => KernelLanguage::JavaScript, KernelLanguage::JavaScript => KernelLanguage::Python };
    if journey.query(&a, other, &variable, "independent-language-while-first-blocked")? {
        return Err("another interpreter language inherited the blocked kernel's variable".into());
    }
    match active.try_recv() {
        Err(mpsc::TryRecvError::Empty) => {}
        Err(cause) => return Err(format!("blocked interpreter result channel failed: {cause}")),
        Ok(Err(cause)) => return Err(format!("blocked interpreter refused prematurely: {cause}")),
        Ok(Ok(result)) => return Err(format!("blocking evaluation finished prematurely: ok={}, error={:?}", result.ok, result.error)),
    }
    let forbidden = a.join("cancelled-input.txt");
    let code = match language {
        KernelLanguage::Python => format!("__import__('pathlib').Path({}).write_text({value})", json!(forbidden.to_str().ok_or("cancelled effect path is not UTF-8")?)),
        KernelLanguage::JavaScript => format!("globalThis[{key}]='unexpectedly-executed'"),
    };
    let (cancel, queued) = journey.launch(&a, language, code, None);
    cancel.cancel();
    if journey.receive("cancel-contending-evaluation-before-teardown", &queued).is_ok() {
        return Err("cancelled contending evaluation executed".into());
    }
    if language == KernelLanguage::Python {
        match fs::metadata(&forbidden) {
            Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {}
            Err(cause) => return Err(format!("inspect cancelled evaluation effect: {cause}")),
            Ok(_) => return Err("cancelled Python input persisted its side effect".into()),
        }
    }
    kernel::teardown_scope(&a);
    let cancelled = journey.receive("teardown-active-evaluation", &active)?;
    if !cancelled.cancelled || !cancelled.stdout.text.lines().any(|line| line.trim() == marker) {
        return Err(format!("teardown lost active cancellation or its observed output: {:?}", cancelled.error));
    }
    if language == KernelLanguage::Python {
        assert_absent(journey, &pid_file, "pythonProcessAfterTeardown")?;
    }
    if journey.query(&a, language, &variable, "fresh-generation-after-teardown")? {
        return Err("a fresh generation reused cancelled interpreter state".into());
    }
    let context_pid = a.join("context-cancel.pid");
    let code = match language {
        KernelLanguage::Python => format!("import os\nglobals()[{key}]={value}\n__import__('pathlib').Path({}).write_text(str(os.getpid()))\nos.write(2,{value}.encode())\nprint({value},flush=True)\nwhile True:\n os.write(2,{value}.encode())", json!(context_pid.to_str().ok_or("context PID path is not UTF-8")?)),
        KernelLanguage::JavaScript => format!("globalThis[{key}]={value}; console.log({value}); while(true){{}}"),
    };
    let (sender, ready) = mpsc::channel();
    let (cancel, running) = journey.launch(&a, language, code, Some(sender));
    if let Err(cause) = ready.recv() {
        let actual = match journey.receive("context-cancel-startup", &running) {
            Err(error) => error,
            Ok(value) => format!("evaluation ended before readiness: ok={}, error={:?}", value.ok, value.error),
        };
        return Err(format!("context cancellation readiness failed: {cause}; {actual}"));
    }
    cancel.cancel();
    let result = journey.receive("cancel-running-evaluation-with-native-stderr", &running)?;
    if !result.cancelled || !result.stdout.text.lines().any(|line| line.trim() == marker) {
        return Err("running evaluation did not retain cancellation and its observed output".into());
    }
    if language == KernelLanguage::Python {
        assert_absent(journey, &context_pid, "pythonProcessAfterContextCancellation")?;
    }
    if journey.query(&a, language, &variable, "fresh-generation-after-context-cancellation")? {
        return Err("context cancellation reused the interrupted interpreter namespace".into());
    }
    if !journey.query(&b, language, &other_variable, "unrelated-scope-survives-teardown")? {
        return Err("teardown discarded another scope's persistent namespace".into());
    }
    Ok(())
}
#[test]
#[ignore = "requires an approved native target, clean committed candidate, real Python and Node, and an enforced sandbox"]
fn real_interpreters_isolate_cancel_and_teardown() {
    for language in [KernelLanguage::Python, KernelLanguage::JavaScript] {
        let mut journey = Journey::new(language).expect("create retained real interpreter journey");
        let mut result = source::bind(&mut journey.report).and_then(|()| exercise(&mut journey, language));
        journey.report["sourceSha256"] = json!(format!("{:x}", Sha256::digest(include_bytes!("lifecycle.rs"))));
        if let Err(cause) = journey.cleanup() {
            result = Err(match result { Ok(()) => cause, Err(original) => format!("{original}; cleanup: {cause}") });
        }
        match &result {
            Ok(()) => journey.report["verdict"] = json!("passed"),
            Err(cause) => journey.report["failure"] = json!(cause),
        }
        fs::write(journey.root.join("report.json"), serde_json::to_vec_pretty(&journey.report).expect("serialize native interpreter observations"))
            .expect("retain actual interpreter journey report");
        if let Err(cause) = result { panic!("{cause}; evidence: {}", journey.root.join("report.json").display()); }
    }
}
