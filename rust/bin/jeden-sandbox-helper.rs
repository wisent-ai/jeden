//! Run one command confined to the read and write roots it was given.
//!
//! Each platform's confinement lives in its own module beside this file;
//! a platform with no backend refuses rather than running the command
//! unconfined, because a caller that believes the sandbox is in force is
//! worse off than one that is told it is not.

#[cfg(target_os = "macos")]
#[path = "sandbox_helper/macos.rs"]
mod platform;

#[cfg(target_os = "linux")]
#[path = "sandbox_helper/linux.rs"]
mod platform;

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn main() {
    platform::run_main();
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn main() {
    eprintln!("jeden-sandbox-helper has no sandbox backend for this platform");
    std::process::exit(1);
}
