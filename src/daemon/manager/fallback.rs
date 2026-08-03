//! Fallback daemon manager using the classic double-fork pattern.
//!
//! Only used when systemd is not available. When running under systemd,
//! `serve run` is the entry point — no fork / daemonize code is touched.

use std::fs;
use std::io;
use std::os::unix::io::AsRawFd;
use std::sync::Arc;

use console_info::Ui;

use crate::config::Config;
use crate::daemon::SyncraftServer;
use crate::error::Error;
use crate::log_error;
use crate::log_info;
use crate::log_warn;
use crate::message::{MessageKey, translate};
use crate::paths::SyncraftPaths;

use super::ServiceManager;

pub struct Fallback;

impl ServiceManager for Fallback {
    fn name(&self) -> &'static str {
        "fallback"
    }

    fn is_available(&self) -> bool {
        true
    }

    fn install(&self, _paths: &SyncraftPaths) -> Result<(), Error> {
        let ui = Ui::get();
        ui.error(&translate(&MessageKey::DaemonInstallUnsupported));
        Err(Error::Daemon(
            "install not available in fallback mode".into(),
        ))
    }

    fn uninstall(&self, _paths: &SyncraftPaths) -> Result<(), Error> {
        Ui::get().info(&translate(&MessageKey::DaemonNotRunning));
        Ok(())
    }

    /// Daemonize the process and run the server.
    ///
    /// **This function does not return** on success — the parent exits
    /// and the grandchild runs the server.
    fn start(&self, paths: &SyncraftPaths) -> Result<(), Error> {
        let ui = Ui::get();

        let config = if let Some(config) = Config::load(paths) {
            config
        } else {
            ui.error(&translate(&MessageKey::Uninitialized));
            std::process::exit(0);
        };

        if pid_file_alive(paths) {
            ui.warn(&translate(&MessageKey::DaemonAlreadyRunning));
            std::process::exit(0);
        }

        // Clean stale socket from a previous crashed daemon.
        let sock = paths.socket_path();
        if sock.exists() {
            let _ = fs::remove_file(&sock);
        }

        log_info!("Daemonizing (fallback / double-fork)");

        daemonize(&paths.pid_file(), &paths.log_file_latest());

        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|e| Error::Daemon(format!("Failed to create runtime: {}", e)))?;

        let server = rt
            .block_on(async { SyncraftServer::new(paths, config).await })
            .map_err(|e| Error::Daemon(format!("Failed to create server: {}", e)))?;
        let server = Arc::new(server);
        rt.block_on(server.run())
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
                    if let Ok(pid_str) = fs::read_to_string(paths.pid_file()) {
                        Ui::get().warn(&translate(&MessageKey::DaemonStalePid {
                            pid: pid_str.trim().to_string(),
                            pid_file: paths.pid_file().display().to_string(),
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
                    Ui::get().info(&translate(&MessageKey::DaemonStatus {
                        pid,
                        uptime_secs,
                        address,
                        manager: "fallback",
                    }));
                }
                Ok(crate::daemon::ipc::IpcResponse::Error { message }) => {
                    log_error!("Daemon reports error: {}", message);
                }
                Err(_) => {
                    if pid_file_alive(paths) {
                        Ui::get().warn(&translate(&MessageKey::DaemonAlreadyRunning));
                    } else {
                        Ui::get().info(&translate(&MessageKey::DaemonNotRunning));
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
        let log_file = paths.log_file_latest();
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
fn daemonize(pid_file: &std::path::Path, log_file: &std::path::Path) {
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

    let pid = std::process::id();
    let _ = fs::write(pid_file, pid.to_string());

    let _ = std::env::set_current_dir("/");
    unsafe {
        libc::umask(0o022);
    }

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
