//! Service manager abstraction.
//!
//! A [`ServiceManager`] knows how to install, start, stop, and query the
//! syncraft server through a platform-specific lifecycle mechanism:
//!
//! | Manager     | Platform        | Mechanism              |
//! |-------------|-----------------|------------------------|
//! | [`Systemd`] | Linux + systemd | `systemctl --user`    |
//! | [`Fallback`]| Any Unix        | Double-fork + IPC     |
//!
//! [`detect`] picks the best available manager at runtime.

use crate::error::Error;
use crate::paths::SyncraftPaths;
use std::process::Command;

// ═══════════════════════════════════════════════════════════════════════════
// Trait
// ═══════════════════════════════════════════════════════════════════════════

/// Platform-specific service lifecycle management.
pub trait ServiceManager: Send + Sync {
    /// Human-readable name (e.g. `"systemd"`, `"fallback"`).
    #[allow(dead_code)]
    fn name(&self) -> &'static str;

    /// Whether this manager is usable on the current system.
    fn is_available(&self) -> bool;

    /// Install the service so the platform can manage it.
    ///
    /// For systemd this writes and enables a user unit.
    /// For the fallback this is a no-op.
    fn install(&self, paths: &SyncraftPaths) -> Result<(), Error>;

    /// Uninstall the service.
    fn uninstall(&self, paths: &SyncraftPaths) -> Result<(), Error>;

    /// Start the daemon through the platform's service manager.
    ///
    /// Under systemd this runs `systemctl --user start`.
    /// Under the fallback this daemonizes the current process.
    ///
    /// **Important:** the fallback implementation never returns when
    /// successful — it replaces the current process with the daemon.
    fn start(&self, paths: &SyncraftPaths) -> Result<(), Error>;

    /// Stop a running daemon.
    fn stop(&self, paths: &SyncraftPaths) -> Result<(), Error>;

    /// Print daemon status to stdout.
    fn status(&self, paths: &SyncraftPaths) -> Result<(), Error>;

    /// Tail the daemon's logs in real time.
    ///
    /// Systemd: runs `journalctl --user -u syncraft.service -f`.
    /// Fallback: tails the log file at `syncraft/logs/syncraft.log`.
    fn logs(&self, paths: &SyncraftPaths) -> Result<(), Error>;
}

// ═══════════════════════════════════════════════════════════════════════════
// Detection
// ═══════════════════════════════════════════════════════════════════════════

mod fallback;
mod systemd;

/// Return the best available service manager for the current platform.
///
/// Tries managers in priority order:
/// 1. **systemd** (Linux user session)
/// 2. **fallback** (always available on Unix)
pub fn detect() -> Box<dyn ServiceManager> {
    let systemd = systemd::Systemd;
    if systemd.is_available() {
        return Box::new(systemd);
    }
    Box::new(fallback::Fallback)
}

// ═══════════════════════════════════════════════════════════════════════════
// Helpers
// ═══════════════════════════════════════════════════════════════════════════

/// Run an external command, capturing stdout and stderr.
/// Returns the combined output on failure.
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

/// Check whether a process PID is alive.
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
/// Reads `/proc/<pid>/stat` and `/proc/uptime` on Linux; returns `None` on
/// any error (process gone, /proc not mounted, etc.).
#[cfg(target_os = "linux")]
pub(crate) fn pid_uptime_secs(pid: u32) -> Option<u64> {
    use std::fs;

    // ── Read /proc/<pid>/stat ──────────────────────────────────────
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // Field 22 is starttime, but field 2 (comm) may contain spaces
    // inside parentheses.  Split after the closing `)`.
    let after_comm = stat.rsplit(')').next()?;
    let fields: Vec<&str> = after_comm.split_whitespace().collect();
    // starttime is the 20th field after comm (index 19).
    let starttime = fields.get(19)?.parse::<u64>().ok()?;

    // ── Read /proc/uptime ──────────────────────────────────────────
    let uptime_raw = fs::read_to_string("/proc/uptime").ok()?;
    let sys_uptime = uptime_raw
        .split_whitespace()
        .next()?
        .parse::<f64>()
        .ok()?;

    let ticks_per_sec = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as u64;

    Some(sys_uptime as u64 - (starttime / ticks_per_sec))
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn pid_uptime_secs(_pid: u32) -> Option<u64> {
    None
}
