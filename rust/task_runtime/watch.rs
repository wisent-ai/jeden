//! Waiting for a change inside one directory, told by the kernel instead of
//! read on a clock: kqueue on macOS, inotify on Linux, a change notification
//! on Windows.
//!
//! The watch is armed when it is created. A caller creates it, checks the
//! state it waits for, and only then calls `wait`, so a change landing between
//! the check and the wait still wakes it.

use std::io;
use std::path::Path;

pub(crate) use imp::DirectoryWatch;

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::CString;
    use std::io;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    pub(crate) struct DirectoryWatch {
        queue: libc::c_int,
        directory: libc::c_int,
    }

    impl DirectoryWatch {
        pub(crate) fn new(path: &Path) -> io::Result<Self> {
            let name = CString::new(path.as_os_str().as_bytes())
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
            let directory = unsafe { libc::open(name.as_ptr(), libc::O_EVTONLY) };
            if directory < 0 {
                return Err(io::Error::last_os_error());
            }
            let queue = unsafe { libc::kqueue() };
            if queue < 0 {
                let error = io::Error::last_os_error();
                unsafe { libc::close(directory) };
                return Err(error);
            }
            let change = libc::kevent {
                ident: directory as libc::uintptr_t,
                filter: libc::EVFILT_VNODE,
                flags: libc::EV_ADD | libc::EV_CLEAR,
                fflags: libc::NOTE_WRITE | libc::NOTE_EXTEND | libc::NOTE_ATTRIB,
                data: 0,
                udata: std::ptr::null_mut(),
            };
            let registered = unsafe {
                libc::kevent(queue, &change, 1, std::ptr::null_mut(), 0, std::ptr::null())
            };
            if registered < 0 {
                let error = io::Error::last_os_error();
                unsafe {
                    libc::close(queue);
                    libc::close(directory);
                }
                return Err(error);
            }
            Ok(Self { queue, directory })
        }

        pub(crate) fn wait(&self) -> io::Result<()> {
            let mut event: libc::kevent = unsafe { std::mem::zeroed() };
            loop {
                let count = unsafe {
                    libc::kevent(
                        self.queue,
                        std::ptr::null(),
                        0,
                        &mut event,
                        1,
                        std::ptr::null(),
                    )
                };
                if count >= 0 {
                    return Ok(());
                }
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::Interrupted {
                    return Err(error);
                }
            }
        }
    }

    impl Drop for DirectoryWatch {
        fn drop(&mut self) {
            unsafe {
                libc::close(self.queue);
                libc::close(self.directory);
            }
        }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use std::ffi::CString;
    use std::io;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    pub(crate) struct DirectoryWatch {
        notify: libc::c_int,
    }

    impl DirectoryWatch {
        pub(crate) fn new(path: &Path) -> io::Result<Self> {
            let name = CString::new(path.as_os_str().as_bytes())
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
            let notify = unsafe { libc::inotify_init1(libc::IN_CLOEXEC) };
            if notify < 0 {
                return Err(io::Error::last_os_error());
            }
            // Entries appearing, renamed in or removed: records are written by
            // rename, so appends to logs in the same directory do not wake it.
            let mask = libc::IN_CREATE | libc::IN_MOVED_TO | libc::IN_DELETE;
            if unsafe { libc::inotify_add_watch(notify, name.as_ptr(), mask) } < 0 {
                let error = io::Error::last_os_error();
                unsafe { libc::close(notify) };
                return Err(error);
            }
            Ok(Self { notify })
        }

        pub(crate) fn wait(&self) -> io::Result<()> {
            let mut buffer = [0u8; 4096];
            loop {
                let read = unsafe {
                    libc::read(
                        self.notify,
                        buffer.as_mut_ptr() as *mut libc::c_void,
                        buffer.len(),
                    )
                };
                if read >= 0 {
                    return Ok(());
                }
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::Interrupted {
                    return Err(error);
                }
            }
        }
    }

    impl Drop for DirectoryWatch {
        fn drop(&mut self) {
            unsafe { libc::close(self.notify) };
        }
    }
}

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    type Handle = *mut c_void;
    const INVALID_HANDLE_VALUE: Handle = -1isize as Handle;
    const FILE_NOTIFY_CHANGE_FILE_NAME: u32 = 0x1;
    const FILE_NOTIFY_CHANGE_LAST_WRITE: u32 = 0x10;
    const INFINITE: u32 = 0xFFFF_FFFF;
    const WAIT_OBJECT_0: u32 = 0;

    #[link(name = "kernel32")]
    extern "system" {
        fn FindFirstChangeNotificationW(path: *const u16, subtree: i32, filter: u32) -> Handle;
        fn FindNextChangeNotification(handle: Handle) -> i32;
        fn FindCloseChangeNotification(handle: Handle) -> i32;
        fn WaitForSingleObject(handle: Handle, milliseconds: u32) -> u32;
    }

    pub(crate) struct DirectoryWatch {
        handle: Handle,
    }

    // The change handle is owned by this value and only used through it.
    unsafe impl Send for DirectoryWatch {}

    impl DirectoryWatch {
        pub(crate) fn new(path: &Path) -> io::Result<Self> {
            let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            let handle = unsafe {
                FindFirstChangeNotificationW(
                    wide.as_ptr(),
                    0,
                    FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_LAST_WRITE,
                )
            };
            if handle == INVALID_HANDLE_VALUE {
                return Err(io::Error::last_os_error());
            }
            Ok(Self { handle })
        }

        /// Blocks until the directory changes, then re-arms for the next change.
        pub(crate) fn wait(&self) -> io::Result<()> {
            if unsafe { WaitForSingleObject(self.handle, INFINITE) } != WAIT_OBJECT_0 {
                return Err(io::Error::last_os_error());
            }
            if unsafe { FindNextChangeNotification(self.handle) } == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }
    }

    impl Drop for DirectoryWatch {
        fn drop(&mut self) {
            unsafe { FindCloseChangeNotification(self.handle) };
        }
    }
}

/// Opens a watch on `dir`, naming the directory in the error.
pub(crate) fn watch(dir: &Path) -> Result<DirectoryWatch, io::Error> {
    DirectoryWatch::new(dir).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("cannot watch {} for changes: {error}", dir.display()),
        )
    })
}
