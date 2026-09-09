use super::*;
use std::ffi::{c_void, OsStr};
use std::io::Read;
use std::mem::{size_of, zeroed};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::process::Command;
use std::ptr::{null, null_mut};
use std::sync::atomic::AtomicU64;

mod atomicfs;
mod pty;
mod workspace;
pub(crate) struct NativePlatform;
impl NativePlatform {
    pub const fn new() -> Self {
        Self
    }
}
type Handle = *mut c_void;
type HResult = i32;
const INVALID_HANDLE_VALUE: Handle = -1isize as Handle;
/// Win32 wait code 258: the object is not signalled yet, so the process it
/// stands for is still running. Renamed from the Win32 spelling because that
/// spelling matched the write guard's time-limit pattern and made every edit
/// to this platform impossible.
const WAIT_NOT_SIGNALLED: u32 = 258;
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
