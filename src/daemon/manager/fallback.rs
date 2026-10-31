//! Fallback daemon manager using the classic double-fork pattern.
//!
//! Only used when systemd is not available. When running under systemd,
//! `daemon foreground` is the entry point — no fork / daemonize code is touched.

use std::fs;
use std::io;
use std::os::unix::io::AsRawFd;

use console_info::Ui;

use crate::daemon::registry::Registry;
use crate::daemon::{prepare_logs, run_daemon};
use crate::error::Error;
use crate::log_error;
use crate::log_info;
use crate::log_warn;
use crate::message::{MessageKey, translate};
use crate::paths::GlobalPaths;

use super::{ServiceManager, print_status};

pub struct Fallback;

impl ServiceManager for Fallback {
    fn name(&self) -> &'static str {
        "fallback"
    }

    fn is_available(&self) -> bool {
        true
    }

    fn install(&self, _global: &GlobalPaths) -> Result<(), Error> {
        let ui = Ui::get();
        ui.error(&translate(&MessageKey::DaemonInstallUnsupported));
        Err(Error::Daemon(
            "install not available in fallback mode".into(),
        ))
    }

    fn uninstall(&self, _global: &GlobalPaths) -> Result<(), Error> {
        Ui::get().info(&translate(&MessageKey::DaemonNotRunning));
        Ok(())
    }

    /// Daemonize the process and run the multi-tenant daemon.
    ///
    /// **This function does not return** on success — the parent exits
    /// and the grandchild runs the daemon.
    fn start(&self, global: &GlobalPaths) -> Result<(), Error> {
        let ui = Ui::get();

        // Validate the registry before forking so a broken registry does not
        // leave a daemon that immediately exits.
        Registry::load(global)?;

        if pid_file_alive(global) {
            ui.warn(&translate(&MessageKey::DaemonAlreadyRunning));
            std::process::exit(0);
        }

        // Clean stale socket from a previous crashed daemon.
        let sock = global.socket_path();
        if sock.exists() {
            let _ = fs::remove_file(&sock);
        }

        prepare_logs(global)?;
        log_info!("Daemonizing (fallback / double-fork)");

        daemonize(&global.log_file_latest(), &global.root());

        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|e| Error::Daemon(format!("Failed to create runtime: {}", e)))?;

        rt.block_on(run_daemon(global))
    }

    fn stop(&self, global: &GlobalPaths) -> Result<(), Error> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| Error::Daemon(format!("Failed to create runtime: {}", e)))?;

        rt.block_on(async {
            match crate::daemon::ipc::send_request(
                &global.socket_path(),
                &crate::daemon::ipc::IpcRequest::Stop,
            )
            .await
            {
                Ok(crate::daemon::ipc::IpcResponse::Ok) => {
                    Ui::get().ok(&translate(&MessageKey::DaemonStopped));
                }
                Ok(crate::daemon::ipc::IpcResponse::Error { message }) => {
                    log_error!("Daemon reported error: {}", message);
                    Ui::get().error(&translate(&MessageKey::Failed { reason: message }));
                }
                Err(e) => {
                    let reason = e.to_string();
                    log_warn!("Failed to contact daemon: {}", reason);
                    Ui::get().error(&translate(&MessageKey::DaemonIpcError { reason }));
                    if let Ok(pid_str) = fs::read_to_string(global.pid_file()) {
                        Ui::get().warn(&translate(&MessageKey::DaemonStalePid {
                            pid: pid_str.trim().to_string(),
                            pid_file: global.pid_file().display().to_string(),
                        }));
                    }
                }
                _ => {
                    log_warn!("Unexpected IPC response from daemon");
                }
            }
            Ok(())
        })
    }

    fn status(&self, global: &GlobalPaths) -> Result<(), Error> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| Error::Daemon(format!("Failed to create runtime: {}", e)))?;

        rt.block_on(async {
            match crate::daemon::ipc::send_request(
                &global.socket_path(),
                &crate::daemon::ipc::IpcRequest::Status,
            )
            .await
            {
                Ok(crate::daemon::ipc::IpcResponse::Status {
                    pid,
                    uptime_secs,
                    servers,
                }) => {
                    print_status(pid, uptime_secs, &servers, "fallback");
                }
                Ok(crate::daemon::ipc::IpcResponse::Error { message }) => {
                    log_error!("Daemon reports error: {}", message);
                }
                Err(_) => {
                    if pid_file_alive(global) {
                        Ui::get().warn(&translate(&MessageKey::DaemonAlreadyRunning));
                    } else {
                        Ui::get().info(&translate(&MessageKey::DaemonNotRunning));
                        let _ = fs::remove_file(global.pid_file());
                        let _ = fs::remove_file(global.socket_path());
                    }
                }
                _ => {}
            }
            Ok(())
        })
    }

    fn logs(&self, global: &GlobalPaths) -> Result<(), Error> {
        let log_file = global.log_file_latest();
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

/// Double-fork the current process into a proper daemon.
fn daemonize(log_file: &std::path::Path, work_dir: &std::path::Path) {
    if let Some(parent) = log_file.parent() {
        let _ = fs::create_dir_all(parent);
    }

    match unsafe { libc::fork() } {
        -1 => {
            log_error!("First fork failed: {}", io::Error::last_os_error());
            std::process::exit(1);
        }
        0 => {}
        _ => std::process::exit(0),
    }

    if unsafe { libc::setsid() } == -1 {
        log_error!("setsid failed: {}", io::Error::last_os_error());
        std::process::exit(1);
    }

    match unsafe { libc::fork() } {
        -1 => {
            log_error!("Second fork failed: {}", io::Error::last_os_error());
            std::process::exit(1);
        }
        0 => {}
        _ => std::process::exit(0),
    }

    // The daemon is no longer tied to a Minecraft server directory; keep the
    // process in the global daemon directory.
    let _ = std::env::set_current_dir(work_dir);
    unsafe {
        libc::umask(0o022);
    }

    redirect_stdio(log_file);

    log_info!("Daemon started (PID {})", std::process::id());
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
}

/// Return `true` if the global PID file references an alive process.
fn pid_file_alive(global: &GlobalPaths) -> bool {
    match fs::read_to_string(global.pid_file()) {
        Ok(s) => match s.trim().parse::<i32>() {
            Ok(pid) => super::process_exists(pid),
            Err(_) => false,
        },
        Err(_) => false,
    }
}
