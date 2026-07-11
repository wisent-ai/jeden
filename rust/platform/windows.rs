use super::*;
use std::ffi::{c_void, OsStr};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::mem::{size_of, zeroed};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::{CommandExt, ExitStatusExt};
use std::process::{Command, Stdio};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) struct NativePlatform;
impl NativePlatform {
    pub const fn new() -> Self {
        Self
    }
}
type Handle = *mut c_void;
type HResult = i32;
const INVALID_HANDLE_VALUE: Handle = -1isize as Handle;
const WAIT_TIMEOUT: u32 = 258;
const CREATE_NEW_PROCESS_GROUP: u32 = 0x200;
const EXTENDED_STARTUPINFO_PRESENT: u32 = 0x0008_0000;
const PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE: usize = 0x0002_0016;
const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: u32 = 9;
const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x2000;
const REPLACEFILE_WRITE_THROUGH: u32 = 2;
const MOVEFILE_REPLACE_EXISTING: u32 = 1;
const MOVEFILE_WRITE_THROUGH: u32 = 8;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

fn wide(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(Some(0)).collect()
}
fn last_error() -> PlatformError {
    io::Error::last_os_error().into()
}
fn close_handle(handle: Handle) {
    if !handle.is_null() && handle != INVALID_HANDLE_VALUE {
        unsafe {
            CloseHandle(handle);
        }
    }
}

struct JobTree {
    job: Handle,
}
unsafe impl Send for JobTree {}
impl ProcessTree for JobTree {
    fn signal(&mut self, _: ProcessSignal) -> Result<(), PlatformError> {
        if unsafe { TerminateJobObject(self.job, 1) } == 0 {
            let e = io::Error::last_os_error();
            if e.raw_os_error() != Some(5) {
                return Err(e.into());
            }
        }
        Ok(())
    }
}
impl Drop for JobTree {
    fn drop(&mut self) {
        close_handle(self.job)
    }
}

impl ProcessPlatform for NativePlatform {
    fn configure_command(&self, command: &mut Command) -> Result<(), PlatformError> {
        command.creation_flags(CREATE_NEW_PROCESS_GROUP);
        Ok(())
    }
    fn attach_process_tree(&self, child: &Child) -> Result<Box<dyn ProcessTree>, PlatformError> {
        let job = unsafe { CreateJobObjectW(null(), null()) };
        if job.is_null() {
            return Err(last_error());
        }
        let mut info: JobObjectExtendedLimitInformation = unsafe { zeroed() };
        info.basic.limit_flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if unsafe {
            SetInformationJobObject(
                job,
                JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
                &info as *const _ as *const c_void,
                size_of::<JobObjectExtendedLimitInformation>() as u32,
            )
        } == 0
            || unsafe { AssignProcessToJobObject(job, child.as_raw_handle() as Handle) } == 0
        {
            let e = last_error();
            close_handle(job);
            return Err(e);
        }
        Ok(Box::new(JobTree { job }))
    }
    fn pipe_reader(
        &self,
        pipe: Box<dyn Read + Send>,
    ) -> Result<Box<dyn PipeReader>, PlatformError> {
        Ok(threaded_pipe(pipe))
    }
}

struct WindowsPty {
    process: Handle,
    input: Handle,
    output: Handle,
    pseudo: Handle,
    job: Handle,
    pid: u32,
    exited: Option<u32>,
}
unsafe impl Send for WindowsPty {}
impl PtySession for WindowsPty {
    fn process_id(&self) -> u32 {
        self.pid
    }
    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        let mut offset = 0;
        while offset < bytes.len() {
            let mut written = 0;
            if unsafe {
                WriteFile(
                    self.input,
                    bytes[offset..].as_ptr() as _,
                    (bytes.len() - offset) as u32,
                    &mut written,
                    null_mut(),
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            offset += written as usize;
        }
        Ok(())
    }
    fn read_available(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let mut available = 0;
        if unsafe {
            PeekNamedPipe(
                self.output,
                null_mut(),
                0,
                null_mut(),
                &mut available,
                null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        if available == 0 {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        let mut read = 0;
        if unsafe {
            ReadFile(
                self.output,
                buffer.as_mut_ptr() as _,
                available.min(buffer.len() as u32),
                &mut read,
                null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(read as usize)
    }
    fn resize(&mut self, cols: u16, rows: u16) -> Result<(), PlatformError> {
        let size = Coord {
            x: cols as i16,
            y: rows as i16,
        };
        let hr = unsafe { ResizePseudoConsole(self.pseudo, size) };
        if hr < 0 {
            Err(PlatformError::Process(format!(
                "ResizePseudoConsole failed with HRESULT 0x{:08x}",
                hr as u32
            )))
        } else {
            Ok(())
        }
    }
    fn alive(&mut self) -> Result<bool, PlatformError> {
        Ok(self.exit_status()?.is_none())
    }
    fn exit_status(&mut self) -> Result<Option<ExitStatus>, PlatformError> {
        if let Some(code) = self.exited {
            return Ok(Some(ExitStatus::from_raw(code)));
        }
        if unsafe { WaitForSingleObject(self.process, 0) } == WAIT_TIMEOUT {
            return Ok(None);
        }
        let mut code = 0;
        if unsafe { GetExitCodeProcess(self.process, &mut code) } == 0 {
            return Err(last_error());
        }
        self.exited = Some(code);
        Ok(Some(ExitStatus::from_raw(code)))
    }
    fn signal(&mut self, _: ProcessSignal) -> Result<(), PlatformError> {
        if unsafe { TerminateJobObject(self.job, 1) } == 0 {
            return Err(last_error());
        }
        Ok(())
    }
}
impl Drop for WindowsPty {
    fn drop(&mut self) {
        let _ = self.signal(ProcessSignal::Kill);
        close_handle(self.input);
        close_handle(self.output);
        unsafe { ClosePseudoConsole(self.pseudo) };
        close_handle(self.process);
        close_handle(self.job)
    }
}

impl PtyPlatform for NativePlatform {
    fn spawn_shell(
        &self,
        cwd: &Path,
        cols: u16,
        rows: u16,
    ) -> Result<Box<dyn PtySession>, PlatformError> {
        unsafe {
            let mut sa = SecurityAttributes {
                length: size_of::<SecurityAttributes>() as u32,
                security_descriptor: null_mut(),
                inherit_handle: 1,
            };
            let (mut in_read, mut in_write, mut out_read, mut out_write) =
                (null_mut(), null_mut(), null_mut(), null_mut());
            if CreatePipe(&mut in_read, &mut in_write, &mut sa, 0) == 0
                || CreatePipe(&mut out_read, &mut out_write, &mut sa, 0) == 0
            {
                return Err(last_error());
            }
            SetHandleInformation(in_write, 1, 0);
            SetHandleInformation(out_read, 1, 0);
            let mut pseudo = null_mut();
            let hr = CreatePseudoConsole(
                Coord {
                    x: cols as i16,
                    y: rows as i16,
                },
                in_read,
                out_write,
                0,
                &mut pseudo,
            );
            close_handle(in_read);
            close_handle(out_write);
            if hr < 0 {
                close_handle(in_write);
                close_handle(out_read);
                return Err(PlatformError::unsupported(
                    "ConPTY",
                    UnsupportedReason::RuntimeApiUnavailable,
                ));
            }
            let mut bytes = 0usize;
            InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut bytes);
            let mut storage = vec![0u8; bytes];
            let attrs = storage.as_mut_ptr() as *mut c_void;
            if InitializeProcThreadAttributeList(attrs, 1, 0, &mut bytes) == 0
                || UpdateProcThreadAttribute(
                    attrs,
                    0,
                    PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE,
                    pseudo,
                    size_of::<Handle>(),
                    null_mut(),
                    null_mut(),
                ) == 0
            {
                ClosePseudoConsole(pseudo);
                close_handle(in_write);
                close_handle(out_read);
                return Err(last_error());
            }
            let mut startup: StartupInfoExW = zeroed();
            startup.startup.cb = size_of::<StartupInfoExW>() as u32;
            startup.attributes = attrs;
            let mut info: ProcessInformation = zeroed();
            let mut cmd = wide(OsStr::new("cmd.exe /Q"));
            let cwd_w = wide(cwd.as_os_str());
            let created = CreateProcessW(
                null(),
                cmd.as_mut_ptr(),
                null_mut(),
                null_mut(),
                0,
                EXTENDED_STARTUPINFO_PRESENT,
                null_mut(),
                cwd_w.as_ptr(),
                &startup.startup,
                &mut info,
            );
            DeleteProcThreadAttributeList(attrs);
            if created == 0 {
                ClosePseudoConsole(pseudo);
                close_handle(in_write);
                close_handle(out_read);
                return Err(last_error());
            }
            close_handle(info.thread);
            let job = CreateJobObjectW(null(), null());
            let mut limits: JobObjectExtendedLimitInformation = zeroed();
            limits.basic.limit_flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if job.is_null()
                || SetInformationJobObject(
                    job,
                    JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
                    &limits as *const _ as _,
                    size_of::<JobObjectExtendedLimitInformation>() as u32,
                ) == 0
                || AssignProcessToJobObject(job, info.process) == 0
            {
                TerminateProcess(info.process, 1);
                close_handle(info.process);
                close_handle(job);
                ClosePseudoConsole(pseudo);
                close_handle(in_write);
                close_handle(out_read);
                return Err(last_error());
            }
            Ok(Box::new(WindowsPty {
                process: info.process,
                input: in_write,
                output: out_read,
                pseudo,
                job,
                pid: info.process_id,
                exited: None,
            }))
        }
    }
    fn startup_handshake(&self) -> (&'static [u8], &'static [u8]) {
        (
            b"@echo off\r\nset JEDEN_READY=__JEDEN_PTY_\r\necho %JEDEN_READY%READY__\r\n",
            b"__JEDEN_PTY_READY__",
        )
    }
    fn command_frame(&self, input: &str, process_id: u32, sequence: u64) -> PtyCommandFrame {
        let marker = format!("__JEDEN_PTY_{process_id}_{sequence}__");
        let bytes =
            format!("{input}\r\nset JEDEN_STATUS=%ERRORLEVEL%\r\necho {marker}:%JEDEN_STATUS%\r\n")
                .into_bytes();
        PtyCommandFrame { marker, bytes }
    }
}

impl WorkspacePlatform for NativePlatform {
    fn isolate(&self, parent: &Path, target: &Path) -> Result<&'static str, PlatformError> {
        if parent.join(".git").exists()
            && quiet(
                Command::new("git")
                    .args(["worktree", "add", "--detach"])
                    .arg(target)
                    .arg("HEAD")
                    .current_dir(parent),
            )
        {
            return Ok("git-worktree");
        }
        copy_tree(parent, target)?;
        Ok("native-copy")
    }
    fn snapshot(
        &self,
        _parent: &Path,
        workspace: &Path,
        max: u64,
    ) -> Result<Vec<u8>, PlatformError> {
        if !workspace.join(".git").exists() {
            return Err(PlatformError::unsupported(
                "non-git workspace snapshot",
                UnsupportedReason::FilesystemCapability,
            ));
        }
        if !quiet(
            Command::new("git")
                .args(["add", "-N", "--all"])
                .current_dir(workspace),
        ) {
            return Err(PlatformError::Process("git add -N failed".into()));
        }
        let out = Command::new("git")
            .args([
                "diff",
                "--binary",
                "--no-ext-diff",
                "--src-prefix=a/",
                "--dst-prefix=b/",
            ])
            .current_dir(workspace)
            .output()?;
        if !out.status.success() {
            return Err(PlatformError::Process(
                String::from_utf8_lossy(&out.stderr).into_owned(),
            ));
        }
        if out.stdout.len() as u64 > max {
            return Err(PlatformError::Process(format!(
                "workspace snapshot exceeds {max} bytes"
            )));
        }
        Ok(out.stdout)
    }
    fn apply_snapshot(
        &self,
        parent: &Path,
        snapshot: &[u8],
        max: u64,
    ) -> Result<(), PlatformError> {
        if snapshot.is_empty() {
            return Ok(());
        }
        let mut child = Command::new("git")
            .args(["apply", "--3way", "--whitespace=nowarn", "-"])
            .current_dir(parent)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        child
            .stdin
            .take()
            .ok_or_else(|| PlatformError::Process("git apply stdin unavailable".into()))?
            .write_all(snapshot)?;
        let out = child.wait_with_output()?;
        if out.stderr.len() as u64 > max {
            return Err(PlatformError::Process(
                "git apply diagnostics exceeded limit".into(),
            ));
        }
        if out.status.success() {
            Ok(())
        } else {
            Err(PlatformError::Process(
                String::from_utf8_lossy(&out.stderr).into_owned(),
            ))
        }
    }
}
impl AtomicFsPlatform for NativePlatform {
    fn create_secure_temp(
        &self,
        directory: &Path,
        prefix: &OsStr,
    ) -> Result<SecureTemp, PlatformError> {
        fs::create_dir_all(directory)?;
        for _ in 0..128 {
            let n = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let mut name = prefix.to_os_string();
            name.push(format!("-{}-{n}.tmp", std::process::id()));
            let path = directory.join(name);
            match OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(file) => {
                    secure_acl(&path)?;
                    return Ok(SecureTemp { path, file });
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            }
        }
        Err(PlatformError::Process(
            "secure temporary-name space exhausted".into(),
        ))
    }
    fn atomic_replace(
        &self,
        staged: &Path,
        destination: &Path,
        backup: Option<&Path>,
    ) -> Result<(), PlatformError> {
        let source = wide(staged.as_os_str());
        let dest = wide(destination.as_os_str());
        if destination.exists() {
            let backup_w = backup.map(|p| wide(p.as_os_str()));
            if unsafe {
                ReplaceFileW(
                    dest.as_ptr(),
                    source.as_ptr(),
                    backup_w.as_ref().map_or(null(), |v| v.as_ptr()),
                    REPLACEFILE_WRITE_THROUGH,
                    null_mut(),
                    null_mut(),
                )
            } == 0
            {
                return Err(last_error());
            }
        } else if unsafe {
            MoveFileExW(
                source.as_ptr(),
                dest.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            return Err(last_error());
        }
        Ok(())
    }
}
impl DesktopPlatform for NativePlatform {
    fn open_path(&self, path: &Path) -> Result<(), PlatformError> {
        let verb = wide(OsStr::new("open"));
        let path = wide(path.as_os_str());
        let result =
            unsafe { ShellExecuteW(null_mut(), verb.as_ptr(), path.as_ptr(), null(), null(), 1) }
                as isize;
        if result <= 32 {
            Err(PlatformError::Process(format!(
                "ShellExecuteW failed with code {result}"
            )))
        } else {
            Ok(())
        }
    }
}
fn secure_acl(path: &Path) -> Result<(), PlatformError> {
    unsafe {
        let sddl = wide(OsStr::new("D:P(A;;FA;;;OW)"));
        let mut sd = null_mut();
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            1,
            &mut sd,
            null_mut(),
        ) == 0
        {
            return Err(last_error());
        }
        let (mut present, mut defaulted, mut acl) = (0, 0, null_mut());
        if GetSecurityDescriptorDacl(sd, &mut present, &mut acl, &mut defaulted) == 0 {
            LocalFree(sd);
            return Err(last_error());
        }
        let p = wide(path.as_os_str());
        let rc = SetNamedSecurityInfoW(
            p.as_ptr() as *mut _,
            1,
            0x8000_0004,
            null_mut(),
            null_mut(),
            acl,
            null_mut(),
        );
        LocalFree(sd);
        if rc != 0 {
            return Err(io::Error::from_raw_os_error(rc as i32).into());
        }
        Ok(())
    }
}
fn quiet(command: &mut Command) -> bool {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}
fn copy_tree(source: &Path, dest: &Path) -> Result<(), PlatformError> {
    fs::create_dir(dest)?;
    for item in fs::read_dir(source)? {
        let item = item?;
        let out = dest.join(item.file_name());
        if item.file_type()?.is_dir() {
            copy_tree(&item.path(), &out)?;
        } else {
            fs::copy(item.path(), out)?;
        }
    }
    Ok(())
}
#[repr(C)]
struct Coord {
    x: i16,
    y: i16,
}
#[repr(C)]
struct SecurityAttributes {
    length: u32,
    security_descriptor: *mut c_void,
    inherit_handle: i32,
}
#[repr(C)]
struct StartupInfoW {
    cb: u32,
    reserved: *mut u16,
    desktop: *mut u16,
    title: *mut u16,
    x: u32,
    y: u32,
    x_size: u32,
    y_size: u32,
    x_chars: u32,
    y_chars: u32,
    fill: u32,
    flags: u32,
    show: u16,
    reserved2: u16,
    reserved2_ptr: *mut u8,
    stdin: Handle,
    stdout: Handle,
    stderr: Handle,
}
#[repr(C)]
struct StartupInfoExW {
    startup: StartupInfoW,
    attributes: *mut c_void,
}
#[repr(C)]
struct ProcessInformation {
    process: Handle,
    thread: Handle,
    process_id: u32,
    thread_id: u32,
}
#[repr(C)]
struct IoCounters {
    read_ops: u64,
    write_ops: u64,
    other_ops: u64,
    read_bytes: u64,
    write_bytes: u64,
    other_bytes: u64,
}
#[repr(C)]
struct BasicLimitInformation {
    per_process_time: i64,
    per_job_time: i64,
    limit_flags: u32,
    min_working: usize,
    max_working: usize,
    active_process_limit: u32,
    affinity: usize,
    priority: u32,
    scheduling: u32,
}
#[repr(C)]
struct JobObjectExtendedLimitInformation {
    basic: BasicLimitInformation,
    io: IoCounters,
    process_memory: usize,
    job_memory: usize,
    peak_process: usize,
    peak_job: usize,
}
#[link(name = "kernel32")]
extern "system" {
    fn CloseHandle(h: Handle) -> i32;
    fn CreateJobObjectW(a: *const c_void, n: *const u16) -> Handle;
    fn SetInformationJobObject(j: Handle, c: u32, i: *const c_void, l: u32) -> i32;
    fn AssignProcessToJobObject(j: Handle, p: Handle) -> i32;
    fn TerminateJobObject(j: Handle, c: u32) -> i32;
    fn CreatePipe(r: *mut Handle, w: *mut Handle, a: *mut SecurityAttributes, s: u32) -> i32;
    fn SetHandleInformation(h: Handle, m: u32, f: u32) -> i32;
    fn CreatePseudoConsole(s: Coord, i: Handle, o: Handle, f: u32, h: *mut Handle) -> HResult;
    fn ResizePseudoConsole(h: Handle, s: Coord) -> HResult;
    fn ClosePseudoConsole(h: Handle);
    fn InitializeProcThreadAttributeList(l: *mut c_void, c: u32, f: u32, s: *mut usize) -> i32;
    fn UpdateProcThreadAttribute(
        l: *mut c_void,
        f: u32,
        a: usize,
        v: Handle,
        s: usize,
        p: *mut c_void,
        r: *mut usize,
    ) -> i32;
    fn DeleteProcThreadAttributeList(l: *mut c_void);
    fn CreateProcessW(
        a: *const u16,
        c: *mut u16,
        pa: *mut c_void,
        ta: *mut c_void,
        inherit: i32,
        flags: u32,
        env: *mut c_void,
        cwd: *const u16,
        start: *const StartupInfoW,
        info: *mut ProcessInformation,
    ) -> i32;
    fn WaitForSingleObject(h: Handle, m: u32) -> u32;
    fn GetExitCodeProcess(h: Handle, c: *mut u32) -> i32;
    fn TerminateProcess(h: Handle, c: u32) -> i32;
    fn WriteFile(h: Handle, b: *const c_void, n: u32, w: *mut u32, o: *mut c_void) -> i32;
    fn ReadFile(h: Handle, b: *mut c_void, n: u32, r: *mut u32, o: *mut c_void) -> i32;
    fn PeekNamedPipe(
        h: Handle,
        b: *mut c_void,
        n: u32,
        r: *mut u32,
        a: *mut u32,
        left: *mut u32,
    ) -> i32;
    fn ReplaceFileW(
        d: *const u16,
        s: *const u16,
        b: *const u16,
        f: u32,
        e: *mut c_void,
        r: *mut c_void,
    ) -> i32;
    fn MoveFileExW(s: *const u16, d: *const u16, f: u32) -> i32;
    fn LocalFree(h: Handle) -> Handle;
}
#[link(name = "advapi32")]
extern "system" {
    fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
        s: *const u16,
        r: u32,
        d: *mut Handle,
        n: *mut u32,
    ) -> i32;
    fn GetSecurityDescriptorDacl(s: Handle, p: *mut i32, a: *mut Handle, d: *mut i32) -> i32;
    fn SetNamedSecurityInfoW(
        n: *mut u16,
        t: u32,
        i: u32,
        o: Handle,
        g: Handle,
        d: Handle,
        s: Handle,
    ) -> u32;
}
#[link(name = "shell32")]
extern "system" {
    fn ShellExecuteW(
        w: Handle,
        o: *const u16,
        f: *const u16,
        p: *const u16,
        d: *const u16,
        s: i32,
    ) -> Handle;
}
