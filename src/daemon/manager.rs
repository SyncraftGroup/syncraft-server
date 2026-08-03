//! Service manager abstraction — picks systemd or a fallback double-fork
//! daemon at runtime via [`detect`].

use crate::error::Error;
use crate::paths::SyncraftPaths;
use std::process::Command;

/// Platform-specific service lifecycle management.
pub trait ServiceManager: Send + Sync {
    /// Human-readable name (e.g. `"systemd"`, `"fallback"`).
    #[allow(dead_code)]
    fn name(&self) -> &'static str;

    /// Whether this manager is usable on the current system.
    fn is_available(&self) -> bool;

    /// Install the service so the platform can manage it.
    fn install(&self, paths: &SyncraftPaths) -> Result<(), Error>;

    /// Uninstall the service.
    fn uninstall(&self, paths: &SyncraftPaths) -> Result<(), Error>;

    /// Start the daemon through the platform's service manager.
    ///
    /// **Important:** the fallback implementation never returns when
    /// successful — it replaces the current process with the daemon.
    fn start(&self, paths: &SyncraftPaths) -> Result<(), Error>;

    /// Stop a running daemon.
    fn stop(&self, paths: &SyncraftPaths) -> Result<(), Error>;

    /// Print daemon status to stdout.
    fn status(&self, paths: &SyncraftPaths) -> Result<(), Error>;

    /// Tail the daemon's logs in real time.
    fn logs(&self, paths: &SyncraftPaths) -> Result<(), Error>;
}

mod fallback;
mod systemd;

/// Return the best available service manager (systemd first, then fallback).
pub fn detect() -> Box<dyn ServiceManager> {
    let systemd = systemd::Systemd;
    if systemd.is_available() {
        return Box::new(systemd);
    }
    Box::new(fallback::Fallback)
}

/// Run an external command, capturing stdout and stderr.
pub(crate) fn run_cmd(program: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| format!("Failed to run `{}`: {}", program, e))?;

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    if output.status.success() {
        Ok(combined)
    } else {
        let code = output.status.code().unwrap_or(-1);
        Err(format!(
            "`{} {}` exited with code {}:\n{}",
            program,
            args.join(" "),
            code,
            combined.trim()
        ))
    }
}

/// Check whether a process PID is alive via `kill(pid, 0)`.
#[cfg(unix)]
pub(crate) fn process_exists(pid: i32) -> bool {
    unsafe { libc::kill(pid, 0) == 0 }
}

#[cfg(not(unix))]
pub(crate) fn process_exists(_pid: i32) -> bool {
    false
}

/// Return the wall-clock uptime of `pid` in seconds.
///
/// Reads `/proc/<pid>/stat` and `/proc/uptime` on Linux; returns `None` on error.
#[cfg(target_os = "linux")]
pub(crate) fn pid_uptime_secs(pid: u32) -> Option<u64> {
    use std::fs;

    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // Field 2 (comm) may contain spaces. Split after the closing `)`.
    let after_comm = stat.rsplit(')').next()?;
    let fields: Vec<&str> = after_comm.split_whitespace().collect();
    // starttime is the 20th field after comm (index 19).
    let starttime = fields.get(19)?.parse::<u64>().ok()?;

    let uptime_raw = fs::read_to_string("/proc/uptime").ok()?;
    let sys_uptime = uptime_raw.split_whitespace().next()?.parse::<f64>().ok()?;

    let ticks_per_sec = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as u64;

    Some(sys_uptime as u64 - (starttime / ticks_per_sec))
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn pid_uptime_secs(_pid: u32) -> Option<u64> {
    None
}
