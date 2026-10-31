//! systemd user-unit service manager.
//!
//! Installs a single user-scoped unit at `~/.config/systemd/user/syncraft.service`.
//! One unit now runs the multi-tenant daemon, so every registered Minecraft
//! server is managed by the same process.

use std::fs;
use std::path::PathBuf;

use console_info::Ui;

use crate::error::Error;
use crate::log_info;
use crate::message::{MessageKey, translate};
use crate::paths::GlobalPaths;

use super::ServiceManager;

const SERVICE_NAME: &str = "syncraft.service";

pub struct Systemd;

impl ServiceManager for Systemd {
    fn name(&self) -> &'static str {
        "systemd"
    }

    fn is_available(&self) -> bool {
        std::process::Command::new("systemctl")
            .args(["--user", "list-units"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    fn install(&self, global: &GlobalPaths) -> Result<(), Error> {
        let ui = Ui::get();
        let unit_path = user_unit_path()?;
        global.create_dirs()?;

        if let Some(parent) = unit_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| Error::Daemon(format!("Failed to create systemd user dir: {}", e)))?;
        }

        let unit = render_unit(global);
        fs::write(&unit_path, &unit).map_err(|e| {
            Error::Daemon(format!(
                "Failed to write unit file {}: {}",
                unit_path.display(),
                e
            ))
        })?;
        log_info!("Systemd unit written to {}", unit_path.display());

        super::run_cmd("systemctl", &["--user", "daemon-reload"]).map_err(Error::Daemon)?;
        super::run_cmd("systemctl", &["--user", "enable", SERVICE_NAME]).map_err(Error::Daemon)?;
        super::run_cmd("systemctl", &["--user", "start", SERVICE_NAME]).map_err(Error::Daemon)?;

        let pid = query_pid().unwrap_or(0);
        log_info!("Systemd service installed and started (PID {pid})");
        ui.ok(&translate(&MessageKey::DaemonStarted { pid }));
        Ok(())
    }

    fn uninstall(&self, _global: &GlobalPaths) -> Result<(), Error> {
        let _ = super::run_cmd("systemctl", &["--user", "stop", SERVICE_NAME]);
        let _ = super::run_cmd("systemctl", &["--user", "disable", SERVICE_NAME]);

        let unit_path = user_unit_path()?;
        if unit_path.exists() {
            fs::remove_file(&unit_path).map_err(|e| {
                Error::Daemon(format!(
                    "Failed to remove unit file {}: {}",
                    unit_path.display(),
                    e
                ))
            })?;
        }

        let _ = super::run_cmd("systemctl", &["--user", "daemon-reload"]);
        log_info!("Systemd service uninstalled");
        Ui::get().ok(&translate(&MessageKey::Done));
        Ok(())
    }

    fn start(&self, _global: &GlobalPaths) -> Result<(), Error> {
        let ui = Ui::get();

        if let Err(e) = super::run_cmd("systemctl", &["--user", "start", SERVICE_NAME]).map(|o| {
            if !o.trim().is_empty() {
                log_info!("systemd: {}", o.trim());
            }
        }) {
            if e.contains("not loaded") || e.contains("not found") {
                Ui::get().error(&translate(&MessageKey::DaemonNotInstalled));
                return Ok(());
            } else {
                return Err(Error::Daemon(e));
            }
        }

        match query_pid() {
            Some(pid) => ui.ok(&translate(&MessageKey::DaemonStarted { pid })),
            None => ui.warn(&translate(&MessageKey::DaemonNotRunning)),
        }
        Ok(())
    }

    fn stop(&self, _global: &GlobalPaths) -> Result<(), Error> {
        let ui = Ui::get();

        match super::run_cmd("systemctl", &["--user", "stop", SERVICE_NAME]) {
            Ok(_) => ui.ok(&translate(&MessageKey::DaemonStopped)),
            Err(e) => {
                if e.contains("not loaded") || e.contains("not running") {
                    ui.info(&translate(&MessageKey::DaemonNotRunning));
                } else {
                    ui.error(&translate(&MessageKey::Failed { reason: e.clone() }));
                    return Err(Error::Daemon(e));
                }
            }
        }
        Ok(())
    }

    fn status(&self, global: &GlobalPaths) -> Result<(), Error> {
        let ui = Ui::get();

        let show = match super::run_cmd(
            "systemctl",
            &[
                "--user",
                "show",
                "-p",
                "ActiveState",
                "-p",
                "MainPID",
                SERVICE_NAME,
            ],
        ) {
            Ok(o) => o,
            Err(_) => {
                ui.info(&translate(&MessageKey::DaemonNotRunning));
                return Ok(());
            }
        };

        let active = show
            .lines()
            .find(|l| l.starts_with("ActiveState="))
            .map(|l| l.trim_start_matches("ActiveState="))
            .unwrap_or("unknown");

        if active != "active" {
            ui.info(&translate(&MessageKey::DaemonNotRunning));
            return Ok(());
        }

        let pid: u32 = show
            .lines()
            .find(|l| l.starts_with("MainPID="))
            .and_then(|l| l.trim_start_matches("MainPID=").parse().ok())
            .unwrap_or(0);

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| Error::Daemon(format!("Failed to create runtime: {}", e)))?;

        let uptime_secs = super::pid_uptime_secs(pid).unwrap_or(0);
        let status = rt.block_on(async {
            crate::daemon::ipc::send_request(
                &global.socket_path(),
                &crate::daemon::ipc::IpcRequest::Status,
            )
            .await
        });

        match status {
            Ok(crate::daemon::ipc::IpcResponse::Status {
                pid,
                uptime_secs,
                servers,
            }) => super::print_status(pid, uptime_secs, &servers, "systemd"),
            _ => super::print_status(pid, uptime_secs, &[], "systemd"),
        }
        Ok(())
    }

    fn logs(&self, global: &GlobalPaths) -> Result<(), Error> {
        // Tail the log file directly — preserves ANSI colours that journald strips.
        let log_file = global.log_file_latest();
        if !log_file.exists() {
            return Err(Error::Daemon(format!(
                "Log file not found: {}\n\
                 The daemon may not have produced any logs yet. \
                 Start it with `syncraft daemon start`.",
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

fn query_pid() -> Option<u32> {
    let output = std::process::Command::new("systemctl")
        .args(["--user", "show", "-p", "MainPID", SERVICE_NAME])
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let pid_str = stdout.strip_prefix("MainPID=")?.trim();
    pid_str.parse::<u32>().ok().filter(|&p| p > 0)
}

fn user_unit_path() -> Result<PathBuf, Error> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| Error::Daemon("Cannot determine HOME directory.".into()))?;
    Ok(home
        .join(".config")
        .join("systemd")
        .join("user")
        .join(SERVICE_NAME))
}

fn render_unit(global: &GlobalPaths) -> String {
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "syncraft".into());

    let cwd = global.root().display().to_string();

    format!(
        r#"# Managed by syncraft. Edit with `syncraft daemon install`.
[Unit]
Description=Syncraft multi-server daemon
After=network-online.target
Wants=network-online.target
Documentation=https://github.com/Falswqwq/syncraft-server

[Service]
Type=simple
ExecStart={exe} daemon foreground
WorkingDirectory={cwd}
Restart=on-failure
RestartSec=5s
Environment=SYNCRAFT_LOG=info
Environment=SYNCRAFT_DATA_DIR={cwd}
StandardOutput=journal
StandardError=journal

[Install]
WantedBy=default.target
"#
    )
}
