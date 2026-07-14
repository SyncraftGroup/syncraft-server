mod manager;
mod sync;
pub(crate) mod ipc;
pub(crate) mod http;

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use crate::daemon::sync::{file_mapping, hash_mapping};
use crate::log_info;

use blake3::Hash;
use syncraft_protocol::SyncFile;
use tokio::sync::{Mutex, Notify};
use tokio::try_join;
use crate::config::Config;
use crate::error::Error;
use crate::paths::SyncraftPaths;
use crate::ui::message::MessageKey;
use crate::ui::Ui;

/// Actions that the `serve` command can take.
pub enum ServeAction {
    Start,
    Stop,
    Status,
    /// Run the server in the foreground (dev / debug / systemd ExecStart).
    Run,
    Install,
    Uninstall,
    /// Tail daemon logs (systemd journal or log file).
    Logs,
}

/// Route to the appropriate handler using the best available service manager.
///
/// `action` is `None` when no subcommand is given; defaults to `Start`.
pub fn handle_command(
    paths: &SyncraftPaths,
    action: Option<&ServeAction>,
) -> Result<(), Error> {
    let action = action.unwrap_or(&ServeAction::Start);

    match action {
        ServeAction::Run => {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .map_err(|e| Error::Daemon(format!("Failed to create runtime: {}", e)))?;
            rt.block_on(cmd_run(paths))
        }

        _ => {
            let mgr = manager::detect();
            match action {
                ServeAction::Start => mgr.start(paths),
                ServeAction::Stop => mgr.stop(paths),
                ServeAction::Status => mgr.status(paths),
                ServeAction::Install => mgr.install(paths),
                ServeAction::Uninstall => mgr.uninstall(paths),
                ServeAction::Logs => mgr.logs(paths),
                ServeAction::Run => unreachable!(),
            }
        }
    }
}

/// Run the server in the foreground — no daemonization.
///
/// Used by `syncraft serve run` and systemd `ExecStart=`.
async fn cmd_run(paths: &SyncraftPaths) -> Result<(), Error> {
    let config =
    if let Some(config) = Config::load(paths) { config }
    else {
        Ui::get().error(MessageKey::Uninitialized);
        return Ok(());
    };

    // Rotate the old log file so `serve logs` always tails the latest session.
    let log_file_path = paths.log_file_latest();
    if log_file_path.exists() {
        let _ = fs::rename(log_file_path, paths.log_file());
    }
    crate::log::init_file(&paths.log_file_latest());

    // Write PID so fallback `stop`/`status` can find the process.
    let pid = std::process::id();
    let _ = fs::write(paths.pid_file(), pid.to_string());
    log_info!("Server starting in foreground (PID {})", pid);

    let server = Arc::new(SyncraftServer::new(paths, config).await?);
    server.run().await
}

pub struct SyncraftServer {
    paths: SyncraftPaths,
    config: Config,
    shutdown: Arc<Notify>,
    file_mapping: Mutex<HashMap<PathBuf, SyncFile>>,
    hash_mapping: Mutex<HashMap<Hash, PathBuf>>
}

impl SyncraftServer {

    pub(crate) async fn new(paths: &SyncraftPaths, config: Config) -> Result<Self, Error> {

        let paths = paths.clone();
        let shutdown = Arc::new(Notify::new());
        let file_mapping = file_mapping(&config).await?;
        let hash_mapping = Mutex::new(hash_mapping(&file_mapping));
        let file_mapping = Mutex::new(file_mapping);

        Ok(Self { paths, config, shutdown, file_mapping, hash_mapping })
    }

    pub(crate) async fn run(self: Arc<Self>) -> Result<(), Error> {

        self.shutdown_handlers();

        try_join!(
            self.run_http(),
            Arc::clone(&self).run_ipc()
        )?;

        log_info!("Server shutdown complete");

        let _ = fs::remove_file(self.paths.socket_path());
        let _ = fs::remove_file(self.paths.pid_file());

        Ok(())
    }

    fn shutdown_handlers(&self) {

        let signal_shutdown = self.shutdown.clone();
        tokio::spawn(async move {
            tokio::signal::ctrl_c().await.ok();
            log_info!("SIGINT received, shutting down");
            signal_shutdown.notify_waiters();
        });

        let term_shutdown = self.shutdown.clone();
        tokio::spawn(async move {
            let mut stream = match tokio::signal::unix::signal(
                tokio::signal::unix::SignalKind::terminate(),
            ) {
                Ok(s) => s,
                Err(_) => return,
            };
            stream.recv().await;
            log_info!("SIGTERM received, shutting down");
            term_shutdown.notify_waiters();
        });
    }
}
