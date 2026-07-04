//! systemd user-unit service manager.
//!
//! Installs a user-scoped systemd unit at
//! `~/.config/systemd/user/syncraft.service` so the server can be managed
//! with `systemctl --user {start,stop,status}`.
//!
//! No root privileges are required — everything runs in the user session.

use std::fs;
use std::path::PathBuf;

use crate::config::Config;
use crate::error::Error;
use crate::log_info;
use crate::paths::SyncraftPaths;
use crate::ui::message::MessageKey;
use crate::ui::Ui;

use super::ServiceManager;

/// Name of the systemd service unit.
const SERVICE_NAME: &str = "syncraft.service";

/// systemd user-unit manager.
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

    fn install(&self, paths: &SyncraftPaths) -> Result<(), Error> {
        let ui = Ui::get();
        let unit_path = user_unit_path()?;

        if let Some(parent) = unit_path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                Error::Daemon(format!("Failed to create systemd user dir: {}", e))
            })?;
        }

        let unit = render_unit(paths);
        fs::write(&unit_path, &unit).map_err(|e| {
            Error::Daemon(format!(
                "Failed to write unit file {}: {}",
                unit_path.display(),
                e
            ))
        })?;
        log_info!("Systemd unit written to {}", unit_path.display());

        super::run_cmd("systemctl", &["--user", "daemon-reload"])
            .map_err(|e| Error::Daemon(e))?;
        super::run_cmd("systemctl", &["--user", "enable", SERVICE_NAME])
            .map_err(|e| Error::Daemon(e))?;
        super::run_cmd("systemctl", &["--user", "start", SERVICE_NAME])
            .map_err(|e| Error::Daemon(e))?;

        let pid = query_pid().unwrap_or(0);
        log_info!("Systemd service installed and started (PID {pid})");
        ui.ok(MessageKey::DaemonStarted { pid });
        Ok(())
    }

    fn uninstall(&self, _paths: &SyncraftPaths) -> Result<(), Error> {
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
        Ui::get().ok(MessageKey::Done);
        Ok(())
    }

    fn start(&self, _paths: &SyncraftPaths) -> Result<(), Error> {
        let ui = Ui::get();

        super::run_cmd("systemctl", &["--user", "start", SERVICE_NAME])
            .map(|o| {
                if !o.trim().is_empty() {
                    log_info!("systemd: {}", o.trim());
                }
            })
            .map_err(|e| {
                ui.error(MessageKey::Failed { reason: e.clone() });
                Error::Daemon(e)
            })?;

        match query_pid() {
            Some(pid) => ui.ok(MessageKey::DaemonStarted { pid }),
            None      => ui.warn(MessageKey::DaemonNotRunning),
        }
        Ok(())
    }

    fn stop(&self, _paths: &SyncraftPaths) -> Result<(), Error> {
        let ui = Ui::get();

        match super::run_cmd("systemctl", &["--user", "stop", SERVICE_NAME]) {
            Ok(_) => ui.ok(MessageKey::DaemonStopped),
            Err(e) => {
                if e.contains("not loaded") || e.contains("not running") {
                    ui.info(MessageKey::DaemonNotRunning);
                } else {
                    ui.error(MessageKey::Failed { reason: e.clone() });
                    return Err(Error::Daemon(e));
                }
            }
        }
        Ok(())
    }

    fn status(&self, paths: &SyncraftPaths) -> Result<(), Error> {
        let ui = Ui::get();

        // Query structured state from systemd.
        let show = match super::run_cmd(
            "systemctl",
            &["--user", "show", "-p", "ActiveState", "-p", "MainPID", SERVICE_NAME],
        ) {
            Ok(o) => o,
            Err(_) => {
                ui.info(MessageKey::DaemonNotRunning);
                return Ok(());
            }
        };

        let active = show
            .lines()
            .find(|l| l.starts_with("ActiveState="))
            .map(|l| l.trim_start_matches("ActiveState="))
            .unwrap_or("unknown");

        if active != "active" {
            ui.info(MessageKey::DaemonNotRunning);
            return Ok(());
        }

        let pid: u32 = show
            .lines()
            .find(|l| l.starts_with("MainPID="))
            .and_then(|l| l.trim_start_matches("MainPID=").parse().ok())
            .unwrap_or(0);

        let uptime_secs = super::pid_uptime_secs(pid).unwrap_or(0);

        let address = Config::load(paths)
            .map(|c| format!("{}:{}", c.network.host.trim(), c.network.port))
            .unwrap_or_else(|| "unknown".into());

        ui.info(MessageKey::DaemonStatus {
            pid,
            uptime_secs,
            address,
            manager: "systemd",
        });
        Ok(())
    }

    fn logs(&self, paths: &SyncraftPaths) -> Result<(), Error> {
        let log_file = paths.log_file();
        if !log_file.exists() {
            return Err(Error::Daemon(format!(
                "Log file not found: {}\n\
                 The daemon may not have produced any logs yet. \
                 Start it with `syncraft serve start`.",
                log_file.display()
            )));
        }

        // Tail the log file directly — preserves ANSI colours that
        // journald would strip.
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
// Helpers
// ═══════════════════════════════════════════════════════════════════════════

/// Fetch the daemon PID from systemd.  Returns `None` if the service is not
/// running or systemd is unavailable.
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

fn render_unit(paths: &SyncraftPaths) -> String {
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "syncraft".into());

    let cwd = paths.root().display().to_string();

    format!(
        r#"# Managed by syncraft. Edit with `syncraft serve install`.
[Unit]
Description=Syncraft Minecraft Server
After=network-online.target
Wants=network-online.target
Documentation=https://github.com/Falswqwq/syncraft-server

[Service]
Type=simple
ExecStart={exe} serve run
WorkingDirectory={cwd}
Restart=on-failure
RestartSec=5s
Environment=SYNCRAFT_LOG=info
StandardOutput=journal
StandardError=journal

[Install]
WantedBy=default.target
"#
    )
}
