pub(crate) mod http;
pub(crate) mod ipc;
mod manager;
pub(crate) mod signing;
pub(crate) mod store;
pub mod sync;

use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::daemon::store::ManifestStore;
use crate::daemon::sync::{HashCache, HashSourceMap, file_mapping};
use crate::git::GitRepository;
use crate::log_error;
use crate::log_info;
use crate::log_warn;

use crate::config::Config;
use crate::error::Error;
use crate::message::{MessageKey, translate};
use crate::paths::SyncraftPaths;
use console_info::Ui;
use syncraft_protocol::{MANIFEST_SCHEMA, Manifest};
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

/// Shutdown signal shared by every server task.
///
/// `Notify::notify_waiters` only wakes tasks that are already waiting, so a
/// signal delivered during startup would be lost. The flag makes the state
/// sticky.
pub(crate) struct Shutdown {
    signalled: AtomicBool,
    notify: Notify,
}

impl Shutdown {
    pub(crate) fn new() -> Self {
        Self {
            signalled: AtomicBool::new(false),
            notify: Notify::new(),
        }
    }

    pub(crate) fn trigger(&self) {
        self.signalled.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    pub(crate) async fn wait(&self) {
        loop {
            if self.signalled.load(Ordering::SeqCst) {
                return;
            }
            let notified = self.notify.notified();
            // Re-check between registering and awaiting to avoid a missed wakeup.
            if self.signalled.load(Ordering::SeqCst) {
                return;
            }
            notified.await;
        }
    }
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

    // Warn when the config does not describe the loader that is installed.
    crate::init::check_server_version(&config, paths);

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
    shutdown: Arc<Shutdown>,
    hash_mapping: Mutex<HashSourceMap>,
    hash_cache: Mutex<HashCache>,
    store: ManifestStore,
    manifest: RwLock<Arc<Manifest>>,
}

struct BuiltManifest {
    manifest: Arc<Manifest>,
    sources: HashSourceMap,
}

/// Rebuild the manifest from the current on-disk state and persist a snapshot.
async fn build_manifest(
    config: &Config,
    paths: &SyncraftPaths,
    repository: &Mutex<GitRepository>,
    hash_cache: &Mutex<HashCache>,
    store: &ManifestStore,
) -> Result<BuiltManifest, Error> {
    let (files, sources) = {
        let mut cache = hash_cache.lock().await;
        file_mapping(config, paths, &mut cache).await?
    };

    let version = config.info.clone();
    // The version hash describes the file set, not the git commit: the same
    // commit can produce different manifests (mod filter, forbid rules).
    let version_hash = Manifest::content_hash(MANIFEST_SCHEMA, &version, &files);
    let commit = match repository.lock().await.latest_hash() {
        Ok(oid) => oid.to_string(),
        Err(e) => {
            log_warn!("Could not read HEAD (workspace not committed yet): {e}");
            String::new()
        }
    };

    let mut manifest = Manifest {
        schema: MANIFEST_SCHEMA,
        version_hash,
        commit,
        version,
        download_concurrency: Some(config.download_concurrency()),
        signature: None,
        files,
    };
    signing::sign_if_enabled(paths, &mut manifest, config.network.sign_manifest)?;

    let manifest = Arc::new(manifest);
    store.persist(&manifest, config.server.snapshot_retention)?;

    Ok(BuiltManifest { manifest, sources })
}

impl SyncraftServer {
    pub(crate) async fn new(paths: &SyncraftPaths, config: Config) -> Result<Self, Error> {
        let paths = paths.clone();
        let repository = Mutex::new(GitRepository::open(&paths.root())?);
        let shutdown = Arc::new(Shutdown::new());
        let store = ManifestStore::new(&paths);
        let hash_cache = Mutex::new(HashCache::load(&paths));

        let built = build_manifest(&config, &paths, &repository, &hash_cache, &store).await?;
        log_info!(
            "Manifest ready: {} files, version {}",
            built.manifest.files.len(),
            built.manifest.version_hash
        );
        if let Some(signature) = &built.manifest.signature {
            log_info!(
                "Manifest signing enabled (ed25519 key fingerprint {})",
                signing::fingerprint(&signature.key)
            );
        } else if config.network.sign_manifest {
            log_warn!("Manifest signing requested but no signature was produced");
        }

        Ok(Self {
            paths,
            config: RwLock::new(config),
            repository,
            shutdown,
            hash_mapping: Mutex::new(built.sources),
            hash_cache,
            store,
            manifest: RwLock::new(built.manifest),
        })
    }

    /// Recompute the manifest from disk (used after `git publish`).
    pub(crate) async fn rebuild_manifest(&self) -> Result<(), Error> {
        let config = self.config.read().await.clone();
        let built = build_manifest(
            &config,
            &self.paths,
            &self.repository,
            &self.hash_cache,
            &self.store,
        )
        .await?;

        *self.hash_mapping.lock().await = built.sources;
        *self.manifest.write().await = built.manifest;
        Ok(())
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
            signal_shutdown.trigger();
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
            term_shutdown.trigger();
        });
    }

    async fn reload_config(self: &Arc<Self>) -> Result<(), Error> {
        let new_config = Config::load(&self.paths).ok_or_else(|| {
            Error::Daemon("Failed to reload config: file not found or invalid".into())
        })?;

        let old_config = self.config.read().await.clone();
        let built = build_manifest(
            &new_config,
            &self.paths,
            &self.repository,
            &self.hash_cache,
            &self.store,
        )
        .await?;

        *self.config.write().await = new_config.clone();
        *self.hash_mapping.lock().await = built.sources;
        *self.manifest.write().await = built.manifest;

        // Listeners and the yggdrasil state are built once at startup.
        if old_config.network.host != new_config.network.host
            || old_config.network.port != new_config.network.port
            || old_config.network.tls != new_config.network.tls
            || old_config.network.sign_manifest != new_config.network.sign_manifest
            || old_config.yggdrasil != new_config.yggdrasil
        {
            log_warn!("network/yggdrasil settings changed; restart the daemon to apply them");
        }

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
                    _ = shutdown.wait() => {
                        break;
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SyncObject;
    use std::path::Path;
    use syncraft_protocol::SyncMode;

    fn commit_all(root: &Path, message: &str) -> git2::Oid {
        let repo = git2::Repository::open(root).unwrap();
        let mut index = repo.index().unwrap();
        index
            .add_all(["*"].iter(), git2::IndexAddOption::FORCE, None)
            .unwrap();
        index.write().unwrap();

        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = git2::Signature::now("syncraft-test", "test@local").unwrap();

        let parents = repo
            .head()
            .ok()
            .and_then(|h| h.target())
            .and_then(|id| repo.find_commit(id).ok());
        let parent_refs: Vec<&git2::Commit> = parents.iter().collect();
        repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parent_refs)
            .unwrap()
    }

    #[tokio::test]
    async fn update_picks_up_published_files() {
        let root = std::env::temp_dir().join("syncraft_test_daemon_update");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("mods")).unwrap();
        std::fs::write(root.join("mods/a.jar"), b"a").unwrap();

        git2::Repository::init(&root).unwrap();
        let first_commit = commit_all(&root, "init");

        let paths = SyncraftPaths::new(root.clone());
        paths.create_dirs().unwrap();

        let config = Config {
            sync: vec![SyncObject {
                source: "mods".into(),
                target: "mods".into(),
                mode: SyncMode::Strict,
                is_mod_dir: false,
                target_is_dir: None,
            }],
            ..Default::default()
        };

        let server = Arc::new(SyncraftServer::new(&paths, config).await.unwrap());
        let first_manifest = server.manifest.read().await.clone();
        assert_eq!(first_manifest.files.len(), 1);
        assert_eq!(first_manifest.version_hash.len(), 64);
        assert_eq!(first_manifest.commit, first_commit.to_string());

        // The initial manifest is snapshotted so its hash can be resolved later.
        assert!(
            server
                .store
                .load(&first_manifest.version_hash)
                .unwrap()
                .is_some()
        );

        // Simulate `git publish`: commit a new file, then tell the daemon to
        // refresh. Deriving changes from `git status()` returns nothing here
        // because the worktree is clean.
        std::fs::write(root.join("mods/b.jar"), b"b").unwrap();
        let second_commit = commit_all(&root, "add b");
        Arc::clone(&server).update().await.unwrap();

        let manifest = server.manifest.read().await.clone();
        assert!(
            manifest.files.contains_key(Path::new("mods/b.jar")),
            "published file missing from manifest: {:?}",
            manifest.files.keys().collect::<Vec<_>>()
        );
        assert_eq!(manifest.files.len(), 2);

        // The content hash identifies the file set, and changes with it.
        assert_ne!(manifest.version_hash, first_manifest.version_hash);
        assert_eq!(manifest.commit, second_commit.to_string());
        assert_eq!(server.store.list().len(), 2);
        assert!(server.store.load(&manifest.version_hash).unwrap().is_some());

        let _ = std::fs::remove_dir_all(&root);
    }
}
