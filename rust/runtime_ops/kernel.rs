use super::{ArtifactSink, BoundedOutput, CancellationToken, OperationContext, OperationProgress, OutputCapture};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{LazyLock, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const FRAME_LIMIT: usize = 64 * 1024;
const POLL: Duration = Duration::from_millis(10);

static KERNELS: LazyLock<Mutex<HashMap<KernelKey, KernelProcess>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum KernelLanguage { Python, JavaScript }

impl KernelLanguage {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value { "python" | "py" => Ok(Self::Python), "javascript" | "js" | "node" => Ok(Self::JavaScript), _ => Err(format!("unsupported kernel language: {value}")) }
    }
    fn label(self) -> &'static str { match self { Self::Python => "python", Self::JavaScript => "javascript" } }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct KernelKey { scope: PathBuf, cwd: PathBuf, language: KernelLanguage }

pub struct KernelResult {
    pub ok: bool,
    pub timed_out: bool,
    pub cancelled: bool,
    pub reset: bool,
    pub stdout: OutputCapture,
    pub stderr: OutputCapture,
    pub display: OutputCapture,
    pub display_mime: Option<String>,
    pub error: Option<String>,
}

pub fn evaluate(
    context: &OperationContext<'_>, scope: &Path, cwd: &Path, language: KernelLanguage,
    code: &str, reset: bool, timeout: Duration,
) -> Result<KernelResult, String> {
    let key = KernelKey { scope: scope.to_path_buf(), cwd: cwd.to_path_buf(), language };
    let mut kernels = KERNELS.lock().map_err(|_| "kernel registry lock poisoned")?;
    if reset { if let Some(mut old) = kernels.remove(&key) { old.terminate(); } }
    let mut kernel = if let Some(mut existing) = kernels.remove(&key) {
        if existing.alive() { existing } else { existing.terminate(); KernelProcess::spawn(language, cwd)? }
    } else {
        KernelProcess::spawn(language, cwd)?
    };
    let outcome = kernel.evaluate(context, code, timeout, reset);
    match outcome {
        Ok((result, healthy)) => { if healthy { kernels.insert(key, kernel); } else { kernel.terminate(); } Ok(result) }
        Err(error) => { kernel.terminate(); Err(error) }
    }
}

pub fn probe(language: KernelLanguage, cwd: &Path) -> Result<(), String> {
    let mut kernel = KernelProcess::spawn(language, cwd)?;
    let artifacts = std::env::temp_dir().join("jeden-kernel-probe-artifacts");
    let context = OperationContext::new(CancellationToken::new(), ArtifactSink::new(artifacts));
    let result = kernel.evaluate(&context, "1", Duration::from_secs(2), true);
    kernel.terminate();
    let (result, _) = result?;
    if result.ok { Ok(()) } else { Err(result.error.unwrap_or_else(|| format!("{} kernel probe failed", language.label()))) }
}

pub fn teardown_scope(scope: &Path) {
    if let Ok(mut kernels) = KERNELS.lock() {
        let keys: Vec<_> = kernels.keys().filter(|key| key.scope == scope).cloned().collect();
        for key in keys { if let Some(mut kernel) = kernels.remove(&key) { kernel.terminate(); } }
    }
}

struct KernelProcess { child: Child, stdin: ChildStdin, stdout: ChildStdout, stderr: ChildStderr, group: u32, language: KernelLanguage, pending: Vec<u8>, sequence: u64 }

impl KernelProcess {
    fn spawn(language: KernelLanguage, cwd: &Path) -> Result<Self, String> {
        let (program, args): (&OsStr, Vec<&OsStr>) = match language {
            KernelLanguage::Python => (OsStr::new("python3"), vec![OsStr::new("-u"), OsStr::new("-c"), OsStr::new(PYTHON_BOOTSTRAP)]),
            KernelLanguage::JavaScript => (OsStr::new("node"), vec![OsStr::new("-e"), OsStr::new(JAVASCRIPT_BOOTSTRAP)]),
        };
        let mut command = Command::new(program);
        command.args(args).current_dir(cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        unsafe { command.pre_exec(|| if setpgid(0, 0) == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }); }
        let mut child = command.spawn().map_err(|error| format!("failed launching {} kernel: {error}", language.label()))?;
        let stdin = child.stdin.take().ok_or("kernel stdin unavailable")?;
        let stdout = child.stdout.take().ok_or("kernel stdout unavailable")?;
        let stderr = child.stderr.take().ok_or("kernel stderr unavailable")?;
        set_nonblocking(stdout.as_raw_fd())?;
        set_nonblocking(stderr.as_raw_fd())?;
        let group = child.id();
        Ok(Self { child, stdin, stdout, stderr, group, language, pending: Vec::with_capacity(8192), sequence: 0 })
    }

    fn alive(&mut self) -> bool { self.child.try_wait().ok().flatten().is_none() }

    fn evaluate(&mut self, context: &OperationContext<'_>, code: &str, timeout: Duration, reset: bool) -> Result<(KernelResult, bool), String> {
        self.sequence = self.sequence.wrapping_add(1);
        let request = json!({"id": self.sequence, "code": code, "timeoutMs": timeout.as_millis().min(u64::MAX as u128) as u64});
        serde_json::to_writer(&mut self.stdin, &request).map_err(|error| error.to_string())?;
        self.stdin.write_all(b"\n").and_then(|_| self.stdin.flush()).map_err(|error| error.to_string())?;
        let mut stdout = BoundedOutput::new("kernel-stdout", context.output_limits(), context.artifacts().clone());
        let mut stderr = BoundedOutput::new("kernel-stderr", context.output_limits(), context.artifacts().clone());
        let mut display = BoundedOutput::new("kernel-display", context.output_limits(), context.artifacts().clone());
        let deadline = context.effective_deadline(timeout);
        let mut display_mime = None;
        let mut progress_total = 0u64;
        loop {
            drain_kernel_stderr(&mut self.stderr, &mut stderr)?;
            if context.cancellation().is_cancelled() { self.interrupt(); return Ok((finish_kernel(false, false, true, reset, stdout, stderr, display, display_mime, Some("kernel evaluation cancelled".into()))?, false)); }
            if Instant::now() >= deadline { self.interrupt(); return Ok((finish_kernel(false, true, false, reset, stdout, stderr, display, display_mime, Some("kernel evaluation timed out".into()))?, false)); }
            if let Some(frame) = self.next_frame()? {
                if frame.get("id").and_then(Value::as_u64) != Some(self.sequence) { continue; }
                match frame.get("type").and_then(Value::as_str).unwrap_or("") {
                    "chunk" => {
                        let bytes = frame.get("data").and_then(Value::as_str).unwrap_or("").as_bytes();
                        match frame.get("stream").and_then(Value::as_str).unwrap_or("") { "stdout" => stdout.write_chunk(bytes), "stderr" => stderr.write_chunk(bytes), "display" => { if display_mime.is_none() { display_mime = frame.get("mime").and_then(Value::as_str).map(ToString::to_string); } display.write_chunk(bytes) }, _ => continue }.map_err(|e| e.to_string())?;
                        progress_total = progress_total.saturating_add(bytes.len() as u64);
                        context.progress(OperationProgress { stream: "kernel", bytes: bytes.len() as u64, total_bytes: progress_total });
                    }
                    "done" => { let ok = frame.get("ok").and_then(Value::as_bool).unwrap_or(false); let error = frame.get("error").and_then(Value::as_str).map(ToString::to_string); return Ok((finish_kernel(ok, false, false, reset, stdout, stderr, display, display_mime, error)?, true)); }
                    _ => {}
                }
            } else if !self.alive() {
                let internal = stderr.finish().map_err(|e|e.to_string())?;
                return Err(format!("{} kernel exited before response: {}", self.language.label(), internal.text));
            } else { thread::sleep(POLL); }
        }
    }

    fn next_frame(&mut self) -> Result<Option<Value>, String> {
        let mut chunk = [0u8; 8192];
        match self.stdout.read(&mut chunk) {
            Ok(0) => return Ok(None),
            Ok(count) => { self.pending.extend_from_slice(&chunk[..count]); if self.pending.len() > FRAME_LIMIT { return Err("kernel protocol frame exceeded 64 KiB".into()); } }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error.to_string()),
        }
        let Some(end) = self.pending.iter().position(|byte| *byte == b'\n') else { return Ok(None); };
        let line: Vec<u8> = self.pending.drain(..=end).collect();
        serde_json::from_slice(&line).map(Some).map_err(|error| format!("invalid kernel protocol frame: {error}"))
    }

    fn interrupt(&mut self) { unsafe { kill(-(self.group as i32), 2); } thread::sleep(Duration::from_millis(100)); }
    fn terminate(&mut self) { unsafe { kill(-(self.group as i32), 15); } let until=Instant::now()+Duration::from_millis(300); while Instant::now()<until { if self.child.try_wait().ok().flatten().is_some(){return;} thread::sleep(POLL); } unsafe { kill(-(self.group as i32), 9); } let _=self.child.wait(); }
}

fn drain_kernel_stderr(reader: &mut ChildStderr, output: &mut BoundedOutput) -> Result<(), String> {
    let mut buffer = [0u8; 8192];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(count) => output.write_chunk(&buffer[..count]).map_err(|e| e.to_string())?,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
            Err(error) => return Err(error.to_string()),
        }
    }
}
fn finish_kernel(ok: bool, timed_out: bool, cancelled: bool, reset: bool, stdout: BoundedOutput, stderr: BoundedOutput, display: BoundedOutput, mime: Option<String>, error: Option<String>) -> Result<KernelResult,String> {
    Ok(KernelResult { ok, timed_out, cancelled, reset, stdout: stdout.finish().map_err(|e|e.to_string())?, stderr: stderr.finish().map_err(|e|e.to_string())?, display: display.finish().map_err(|e|e.to_string())?, display_mime: mime, error })
}

fn set_nonblocking(fd: i32) -> Result<(), String> { let flags=unsafe{fcntl(fd,3,0)}; if flags<0{return Err(io::Error::last_os_error().to_string());} if unsafe{fcntl(fd,4,flags|4)}<0{return Err(io::Error::last_os_error().to_string());} Ok(()) }

extern "C" { fn setpgid(pid:i32,pgid:i32)->i32; fn kill(pid:i32,signal:i32)->i32; fn fcntl(fd:i32,cmd:i32,...)->i32; }

const PYTHON_BOOTSTRAP: &str = r#"import sys,json,traceback,base64
G={'__name__':'__main__'}
def emit(i,s,d,m=None):
 for p in [d[x:x+3072] for x in range(0,len(d),3072)] or ['']:
  print(json.dumps({'id':i,'type':'chunk','stream':s,'data':p,'mime':m}),file=sys.__stdout__,flush=True)
class W:
 def __init__(self,i,s): self.i=i; self.s=s
 def write(self,d):
  if d: emit(self.i,self.s,str(d))
 def flush(self): pass
for line in sys.stdin:
 try:
  r=json.loads(line); i=r['id']; code=r['code']; oldo,olde=sys.stdout,sys.stderr; sys.stdout,sys.stderr=W(i,'stdout'),W(i,'stderr')
  try:
   try: v=eval(compile(code,'<jeden>','eval'),G,G)
   except SyntaxError: exec(compile(code,'<jeden>','exec'),G,G); v=None
   if v is not None:
    if hasattr(v,'_repr_png_'):
     p=v._repr_png_(); emit(i,'display',base64.b64encode(p).decode() if isinstance(p,bytes) else str(p),'image/png;base64')
    elif hasattr(v,'_repr_html_'): emit(i,'display',str(v._repr_html_()),'text/html')
    elif hasattr(v,'_repr_json_'): emit(i,'display',json.dumps(v._repr_json_()),'application/json')
    else: emit(i,'display',repr(v),'text/plain')
   done={'id':i,'type':'done','ok':True}
  except BaseException as e:
   traceback.print_exc(); done={'id':i,'type':'done','ok':False,'error':str(e)}
  finally: sys.stdout,sys.stderr=oldo,olde
  print(json.dumps(done),flush=True)
 except BaseException as e: print(json.dumps({'id':0,'type':'done','ok':False,'error':str(e)}),flush=True)
"#;

const JAVASCRIPT_BOOTSTRAP: &str = r#"const vm=require('vm'),readline=require('readline'),util=require('util');let id=0;function emit(s,d,m){d=String(d);for(let x=0;x<d.length;x+=3072)process.stdout.write(JSON.stringify({id,type:'chunk',stream:s,data:d.slice(x,x+3072),mime:m})+'\n')}const context=vm.createContext({});context.console={log:(...a)=>emit('stdout',a.map(x=>typeof x==='string'?x:util.inspect(x)).join(' ')+'\n'),error:(...a)=>emit('stderr',a.map(x=>typeof x==='string'?x:util.inspect(x)).join(' ')+'\n')};readline.createInterface({input:process.stdin}).on('line',line=>{try{const r=JSON.parse(line);id=r.id;let v=vm.runInContext(r.code,context,{timeout:Math.max(1,r.timeoutMs)});if(v!==undefined)emit('display',typeof v==='string'?v:util.inspect(v,{depth:4}),'text/plain');process.stdout.write(JSON.stringify({id,type:'done',ok:true})+'\n')}catch(e){emit('stderr',e.stack||String(e));process.stdout.write(JSON.stringify({id,type:'done',ok:false,error:String(e.message||e)})+'\n')}});
"#;
