//! Fallback daemon manager for systems without a native service manager.
//!
//! Implements the classic double-fork daemonization pattern:
//!
//! 1. Validate workspace, check no existing daemon.
//! 2. `fork()` → parent exits.
//! 3. `setsid()` → child becomes session leader.
//! 4. `fork()` again → grandchild can never acquire a controlling terminal.
//! 5. `chdir("/")`, reset umask, redirect stdio to log file.
//! 6. Write PID file.
//! 7. Start a fresh Tokio runtime and run the server.
//!
//! # Independence from systemd
//!
//! This module is **only** loaded and used when systemd is not available.
//! When the server runs under systemd, `serve run` is the entry point —
//! no fork / daemonize / PID-file code is ever touched.

use std::fs;
use std::io;
use std::os::unix::io::AsRawFd;

use crate::config::Config;
use crate::log_error;
use crate::log_info;
use crate::log_warn;
use crate::error::Error;
use crate::paths::SyncraftPaths;
use crate::ui::message::MessageKey;
use crate::ui::Ui;

use super::ServiceManager;

/// Fallback manager — always says it's available.
pub struct Fallback;

impl ServiceManager for Fallback {
    fn name(&self) -> &'static str {
        "fallback"
    }

    fn is_available(&self) -> bool {
        true // Last resort, always available on Unix.
    }

    fn install(&self, _paths: &SyncraftPaths) -> Result<(), Error> {
        let ui = Ui::get();
        ui.error(MessageKey::DaemonInstallUnsupported);
        Err(Error::Daemon("install not available in fallback mode".into()))
    }

    fn uninstall(&self, _paths: &SyncraftPaths) -> Result<(), Error> {
        Ui::get().info(MessageKey::DaemonNotRunning);
        Ok(())
    }

    /// Daemonize the process and run the server.
    ///
    /// **This function does not return** on success — the parent exits,
    /// and the grandchild continues into [`crate::daemon::run_server`].
    fn start(&self, paths: &SyncraftPaths) -> Result<(), Error> {
        let ui = Ui::get();

        // ── Pre-flight checks ────────────────────────────────────
        let config =
        if let Some(config) = Config::load(paths) { config }
        else {
            ui.error(MessageKey::Uninitialized);
            std::process::exit(0);
        };

        if pid_file_alive(paths) {
            ui.warn(MessageKey::DaemonAlreadyRunning);
            std::process::exit(0);
        }

        // Clean stale socket from a previous crashed daemon.
        let sock = paths.socket_path();
        if sock.exists() {
            let _ = fs::remove_file(&sock);
        }

        log_info!("Daemonizing (fallback / double-fork)");

        // ── Daemonize ────────────────────────────────────────────
        daemonize(&paths.pid_file(), &paths.log_file());

        // From here on we are the daemon grandchild.
        // No Tokio runtime has been started yet — safe to do so now.
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|e| Error::Daemon(format!("Failed to create runtime: {}", e)))?;

        rt.block_on(crate::daemon::run_server(paths, config))
    }

    fn stop(&self, paths: &SyncraftPaths) -> Result<(), Error> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| Error::Daemon(format!("Failed to create runtime: {}", e)))?;

        rt.block_on(async {
            match crate::daemon::ipc::send_request(
                &paths.socket_path(),
                &crate::daemon::ipc::IpcRequest::Stop,
            )
            .await
            {
                Ok(crate::daemon::ipc::IpcResponse::Ok) => {
                    Ui::get().ok(MessageKey::DaemonStopped);
                }
                Ok(crate::daemon::ipc::IpcResponse::Error { message }) => {
                    log_error!("Daemon reported error: {}", message);
                    Ui::get().error(MessageKey::Failed { reason: message });
                }
                Err(e) => {
                    let reason = e.to_string();
                    log_warn!("Failed to contact daemon: {}", reason);
                    Ui::get().error(MessageKey::DaemonIpcError { reason });
                    if let Ok(pid_str) = fs::read_to_string(paths.pid_file()) {
                        Ui::get().warn(MessageKey::DaemonStalePid {
                            pid: pid_str.trim().to_string(),
                            pid_file: paths.pid_file().display().to_string(),
                        });
                    }
                }
                _ => {
                    log_warn!("Unexpected IPC response from daemon");
                }
            }
            Ok(())
        })
    }

    fn status(&self, paths: &SyncraftPaths) -> Result<(), Error> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| Error::Daemon(format!("Failed to create runtime: {}", e)))?;

        rt.block_on(async {
            match crate::daemon::ipc::send_request(
                &paths.socket_path(),
                &crate::daemon::ipc::IpcRequest::Status,
            )
            .await
            {
                Ok(crate::daemon::ipc::IpcResponse::Status { pid, uptime_secs }) => {
                    let address = Config::load(paths)
                        .map(|c| format!("{}:{}", c.network.host.trim(), c.network.port))
                        .unwrap_or_else(|| "unknown".into());
                    Ui::get().info(MessageKey::DaemonStatus {
                        pid,
                        uptime_secs,
                        address,
                        manager: "fallback",
                    });
                }
                Ok(crate::daemon::ipc::IpcResponse::Error { message }) => {
                    log_error!("Daemon reports error: {}", message);
                }
                Err(_) => {
                    if pid_file_alive(paths) {
                        Ui::get().warn(MessageKey::DaemonAlreadyRunning);
                    } else {
                        Ui::get().info(MessageKey::DaemonNotRunning);
                        let _ = fs::remove_file(paths.pid_file());
                        let _ = fs::remove_file(paths.socket_path());
                    }
                }
                _ => {}
            }
            Ok(())
        })
    }

    fn logs(&self, paths: &SyncraftPaths) -> Result<(), Error> {
        let log_file = paths.log_file();
        if !log_file.exists() {
            return Err(Error::Daemon(format!(
                "Log file not found: {}",
                log_file.display()
            )));
        }

        use std::process::Command;
        let status = Command::new("tail")
            .args(["-f", &log_file.to_string_lossy()])
            .status()
            .map_err(|e| Error::Daemon(format!("Failed to run tail: {e}")))?;

        if !status.success() {
            return Err(Error::Daemon("tail exited with error".into()));
        }
        Ok(())
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Daemonization (Linux double-fork)
// ═══════════════════════════════════════════════════════════════════════════

/// Double-fork the current process into a proper daemon.
fn daemonize(pid_file: &std::path::Path, log_file: &std::path::Path) {
    // Ensure log directory exists before we lose cwd.
    if let Some(parent) = log_file.parent() {
        let _ = fs::create_dir_all(parent);
    }

    // ── First fork ──────────────────────────────────────────────
    match unsafe { libc::fork() } {
        -1 => {
            log_error!("First fork failed: {}", io::Error::last_os_error());
            std::process::exit(1);
        }
        0 => { /* child continues */ }
        _ => { /* parent exits */ std::process::exit(0); }
    }

    // ── Create new session ──────────────────────────────────────
    if unsafe { libc::setsid() } == -1 {
        log_error!("setsid failed: {}", io::Error::last_os_error());
        std::process::exit(1);
    }

    // ── Second fork ─────────────────────────────────────────────
    match unsafe { libc::fork() } {
        -1 => {
            log_error!("Second fork failed: {}", io::Error::last_os_error());
            std::process::exit(1);
        }
        0 => { /* grandchild continues */ }
        _ => { /* first child exits */ std::process::exit(0); }
    }

    // ── Write PID file ──────────────────────────────────────────
    let pid = std::process::id();
    let _ = fs::write(pid_file, pid.to_string());

    // ── chdir("/"), reset umask ─────────────────────────────────
    let _ = std::env::set_current_dir("/");
    unsafe { libc::umask(0o022); }

    // ── Redirect stdin / stdout / stderr to log file ────────────
    redirect_stdio(log_file);

    log_info!("Daemon started (PID {})", pid);
}

/// Redirect file descriptors 0, 1, 2 to `log_file`.
fn redirect_stdio(log_file: &std::path::Path) {
    let Ok(file) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_file)
    else {
        return;
    };

    let fd = file.as_raw_fd();
    unsafe {
        libc::dup2(fd, libc::STDIN_FILENO);
        libc::dup2(fd, libc::STDOUT_FILENO);
        libc::dup2(fd, libc::STDERR_FILENO);
    }
    let _ = file;

    // The logger will now write to the log file via stderr.
    // No manual write needed.
}

/// Return `true` if the PID file references an alive process.
fn pid_file_alive(paths: &SyncraftPaths) -> bool {
    match fs::read_to_string(paths.pid_file()) {
        Ok(s) => match s.trim().parse::<i32>() {
            Ok(pid) => super::process_exists(pid),
            Err(_) => false,
        },
        Err(_) => false,
    }
}
