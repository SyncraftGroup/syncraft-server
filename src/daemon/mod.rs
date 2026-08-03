pub(crate) mod http;
pub(crate) mod ipc;
mod manager;
pub mod sync;

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::daemon::sync::{file_mapping, hash_mapping};
use crate::git::GitRepository;
use crate::log_info;
use crate::log_error;

use crate::config::Config;
use crate::error::Error;
use crate::message::{MessageKey, translate};
use crate::paths::SyncraftPaths;
use blake3::Hash;
use console_info::Ui;
use syncraft_protocol::{Manifest, SyncFile};
use tokio::sync::{Mutex, Notify, RwLock};
use tokio::try_join;

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
pub fn handle_command(paths: &SyncraftPaths, action: Option<&ServeAction>) -> Result<(), Error> {
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
    let config = if let Some(config) = Config::load(paths) {
        config
    } else {
        Ui::get().error(&translate(&MessageKey::Uninitialized));
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
    config: RwLock<Config>,
    repository: Mutex<GitRepository>,
    shutdown: Arc<Notify>,
    file_mapping: Mutex<HashMap<PathBuf, SyncFile>>,
    hash_mapping: Mutex<HashMap<Hash, PathBuf>>,
    manifest: RwLock<Arc<Manifest>>,
}

impl SyncraftServer {
    pub(crate) async fn new(paths: &SyncraftPaths, config: Config) -> Result<Self, Error> {
        let paths = paths.clone();
        let repository = Mutex::new(GitRepository::open(&paths.root())?);
        let shutdown = Arc::new(Notify::new());
        let file_mapping = file_mapping(&config, &paths).await?;
        let hash_mapping = Mutex::new(hash_mapping(&file_mapping));

        let manifest = {
            let version_hash = repository.lock().await.latest_hash()?.to_string();
            let version = config.info.clone();
            let files = file_mapping.clone();
            Arc::new(Manifest {
                version_hash,
                version,
                files,
            })
        };

        let file_mapping = Mutex::new(file_mapping);
        let manifest = RwLock::new(manifest);

        Ok(Self {
            paths,
            config: RwLock::new(config),
            repository,
            shutdown,
            file_mapping,
            hash_mapping,
            manifest,
        })
    }

    pub(crate) async fn run(self: Arc<Self>) -> Result<(), Error> {
        self.shutdown_handlers();
        self.spawn_config_watcher();

        try_join!(Arc::clone(&self).run_http(), Arc::clone(&self).run_ipc())?;

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
            let mut stream =
                match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                    Ok(s) => s,
                    Err(_) => return,
                };
            stream.recv().await;
            log_info!("SIGTERM received, shutting down");
            term_shutdown.notify_waiters();
        });
    }

    async fn reload_config(self: &Arc<Self>) -> Result<(), Error> {
        let new_config = Config::load(&self.paths).ok_or_else(|| {
            Error::Daemon("Failed to reload config: file not found or invalid".into())
        })?;

        let file_mapping = file_mapping(&new_config, &self.paths).await?;
        let hash_mapping = hash_mapping(&file_mapping);

        let version_hash = self.repository.lock().await.latest_hash()?.to_string();
        let version = new_config.info.clone();
        let files = file_mapping.clone();

        let manifest = Arc::new(Manifest {
            version_hash,
            version,
            files,
        });

        *self.config.write().await = new_config;
        *self.file_mapping.lock().await = file_mapping;
        *self.hash_mapping.lock().await = hash_mapping;
        *self.manifest.write().await = manifest;

        log_info!("Config reloaded successfully");
        Ok(())
    }

    fn spawn_config_watcher(self: &Arc<Self>) {
        let this = Arc::clone(self);
        let shutdown = self.shutdown.clone();
        let config_path = self.paths.config_path();

        tokio::spawn(async move {
            let mut last_mtime = std::fs::metadata(&config_path)
                .and_then(|m| m.modified())
                .ok();

            loop {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(5)) => {
                        let current_mtime = std::fs::metadata(&config_path)
                            .and_then(|m| m.modified())
                            .ok();

                        if current_mtime.is_some() && current_mtime != last_mtime {
                            last_mtime = current_mtime;
                            log_info!("Config file changed, reloading...");
                            if let Err(e) = this.reload_config().await {
                                log_error!("Failed to reload config: {e}");
                            }
                        }
                    }
                    _ = shutdown.notified() => {
                        break;
                    }
                }
            }
        });
    }
}
