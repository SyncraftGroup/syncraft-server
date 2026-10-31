pub(crate) mod http;
pub(crate) mod ipc;
mod manager;
pub mod registry;
pub(crate) mod signing;
pub(crate) mod store;
pub mod sync;

use std::collections::HashMap;
use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::daemon::registry::{Registry, ServerEntry};
use crate::daemon::store::ManifestStore;
use crate::daemon::sync::{HashCache, HashSourceMap, file_mapping};
use crate::git::GitRepository;
use crate::log_error;
use crate::log_info;
use crate::log_warn;

use crate::config::Config;
use crate::error::Error;
use crate::message::{MessageKey, translate};
use crate::paths::{GlobalPaths, SyncraftPaths};
use console_info::Ui;
use syncraft_protocol::{MANIFEST_SCHEMA, Manifest};
use tokio::sync::{Mutex, Notify, RwLock};

/// Actions that the `daemon` command can take.
pub enum DaemonAction {
    Start,
    Stop,
    Status,
    /// Run the daemon in the foreground (systemd ExecStart / debugging).
    Foreground,
    Install,
    Uninstall,
    /// Tail daemon logs.
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
pub fn handle_command(global: &GlobalPaths, action: &DaemonAction) -> Result<(), Error> {
    // `stop` / `status` talk to the daemon over IPC first, so a foreground or
    // fallback daemon is found even on machines where systemd is available.
    if matches!(action, DaemonAction::Stop) && ipc_stop(global)? {
        return Ok(());
    }
    if matches!(action, DaemonAction::Status) && ipc_status(global)? {
        return Ok(());
    }
    if matches!(action, DaemonAction::Start)
        && ipc::is_daemon_alive(&global.socket_path(), &global.pid_file())
    {
        Ui::get().warn(&translate(&MessageKey::DaemonAlreadyRunning));
        return Ok(());
    }

    match action {
        DaemonAction::Foreground => {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .map_err(|e| Error::Daemon(format!("Failed to create runtime: {}", e)))?;
            rt.block_on(run_foreground(global))
        }

        _ => {
            let mgr = manager::detect();
            match action {
                DaemonAction::Start => mgr.start(global),
                DaemonAction::Stop => mgr.stop(global),
                DaemonAction::Status => mgr.status(global),
                DaemonAction::Install => mgr.install(global),
                DaemonAction::Uninstall => mgr.uninstall(global),
                DaemonAction::Logs => mgr.logs(global),
                DaemonAction::Foreground => unreachable!(),
            }
        }
    }
}

/// Try to stop a running daemon via IPC. `Ok(true)` means it was handled.
fn ipc_stop(global: &GlobalPaths) -> Result<bool, Error> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| Error::Daemon(format!("Failed to create runtime: {}", e)))?;

    match rt.block_on(ipc::send_request(
        &global.socket_path(),
        &ipc::IpcRequest::Stop,
    )) {
        Ok(ipc::IpcResponse::Ok) => {
            Ui::get().ok(&translate(&MessageKey::DaemonStopped));
            Ok(true)
        }
        Ok(ipc::IpcResponse::Error { message }) => {
            Ui::get().error(&translate(&MessageKey::Failed { reason: message }));
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// Try to query a running daemon via IPC. `Ok(true)` means it was handled.
fn ipc_status(global: &GlobalPaths) -> Result<bool, Error> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| Error::Daemon(format!("Failed to create runtime: {}", e)))?;

    match rt.block_on(ipc::send_request(
        &global.socket_path(),
        &ipc::IpcRequest::Status,
    )) {
        Ok(ipc::IpcResponse::Status {
            pid,
            uptime_secs,
            servers,
        }) => {
            let manager = manager::detect();
            manager::print_status(pid, uptime_secs, &servers, manager.name());
            Ok(true)
        }
        Ok(ipc::IpcResponse::Error { message }) => {
            log_error!("Daemon reports error: {message}");
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// Run the daemon in the foreground — no daemonization.
///
/// Used by `syncraft daemon foreground` and systemd `ExecStart=`.
async fn run_foreground(global: &GlobalPaths) -> Result<(), Error> {
    prepare_logs(global)?;
    run_daemon(global).await
}

/// Create directories and rotate the previous `latest.log` before opening a
/// fresh one. Must happen before stdio is redirected in fallback mode.
pub(crate) fn prepare_logs(global: &GlobalPaths) -> Result<(), Error> {
    global.create_dirs()?;
    let latest = global.log_file_latest();
    if latest.exists() {
        let _ = fs::rename(&latest, global.log_file());
    }
    Ok(())
}

/// Start the daemon proper. Assumes [`prepare_logs`] already ran.
pub(crate) async fn run_daemon(global: &GlobalPaths) -> Result<(), Error> {
    global.create_dirs()?;
    crate::log::init_file(&global.log_file_latest());

    let pid = std::process::id();
    let _ = fs::write(global.pid_file(), pid.to_string());
    log_info!("Daemon starting (PID {pid})");

    let daemon = Arc::new(Daemon::new(global.clone()).await?);
    daemon.run().await?;

    log_info!("Daemon shutdown complete");
    let _ = fs::remove_file(global.socket_path());
    let _ = fs::remove_file(global.pid_file());
    Ok(())
}

/// One daemon process managing every registered Minecraft server.
pub(crate) struct Daemon {
    pub(crate) global: GlobalPaths,
    pub(crate) shutdown: Arc<Shutdown>,
    pub(crate) instances: Mutex<HashMap<String, Instance>>,
}

pub(crate) struct Instance {
    pub(crate) entry: ServerEntry,
    pub(crate) server: Arc<SyncraftServer>,
    pub(crate) address: String,
}

impl Daemon {
    async fn new(global: GlobalPaths) -> Result<Self, Error> {
        global.create_dirs()?;
        Ok(Self {
            global,
            shutdown: Arc::new(Shutdown::new()),
            instances: Mutex::new(HashMap::new()),
        })
    }

    async fn run(self: Arc<Self>) -> Result<(), Error> {
        let registry = Registry::load(&self.global)?;
        self.apply_registry(&registry, false).await;

        let count = self.instances.lock().await.len();
        if count == 0 {
            log_warn!(
                "No servers registered. Add one with `syncraft server add /path/to/minecraft-server`."
            );
        } else {
            log_info!("{count} server(s) registered");
        }

        let mut ipc = tokio::spawn(Arc::clone(&self).run_ipc());
        tokio::select! {
            _ = self.shutdown.wait() => {
                log_info!("Shutdown requested");
            }
            result = &mut ipc => {
                match result {
                    Ok(Ok(())) => log_warn!("IPC server exited"),
                    Ok(Err(e)) => log_error!("IPC server failed: {e}"),
                    Err(e) => log_error!("IPC task failed: {e}"),
                }
            }
        }

        // Stop every tenant. Each instance has its own shutdown token so it can
        // also be unregistered individually by a reload.
        let servers: Vec<Arc<SyncraftServer>> = self
            .instances
            .lock()
            .await
            .values()
            .map(|instance| Arc::clone(&instance.server))
            .collect();
        for server in servers {
            server.shutdown.trigger();
        }

        if !ipc.is_finished() {
            let _ = tokio::time::timeout(Duration::from_secs(3), ipc).await;
        }
        Ok(())
    }

    /// Start instances that are new and (when `remove_missing`) stop instances
    /// whose registry entry disappeared.
    async fn apply_registry(&self, registry: &Registry, remove_missing: bool) {
        for entry in &registry.servers {
            if self.instances.lock().await.contains_key(&entry.name) {
                continue;
            }
            self.spawn_instance(entry.clone()).await;
        }

        if remove_missing {
            let registered: Vec<&str> = registry
                .servers
                .iter()
                .map(|entry| entry.name.as_str())
                .collect();
            let stale: Vec<String> = self
                .instances
                .lock()
                .await
                .keys()
                .filter(|name| !registered.contains(&name.as_str()))
                .cloned()
                .collect();

            for name in stale {
                if let Some(instance) = self.instances.lock().await.remove(&name) {
                    instance.server.shutdown.trigger();
                    log_info!("[{name}] unregistered");
                }
            }
        }
    }

    async fn spawn_instance(&self, entry: ServerEntry) {
        let paths = SyncraftPaths::new(entry.path.clone());
        let config = match Config::load(&paths) {
            Some(config) => config,
            None => {
                log_error!(
                    "[{}] cannot load {}",
                    entry.name,
                    paths.config_path().display()
                );
                return;
            }
        };

        let address = format!("{}:{}", config.network.host.trim(), config.network.port);
        crate::init::check_server_version(&config, &paths);
        match SyncraftServer::new(&paths, config).await {
            Ok(server) => {
                let server = Arc::new(server);
                server.spawn_config_watcher();

                let task = Arc::clone(&server);
                let name = entry.name.clone();
                tokio::spawn(async move {
                    if let Err(e) = task.run_http().await {
                        log_error!("[{name}] HTTP server stopped: {e}");
                    }
                });

                log_info!("[{}] serving on {address}", entry.name);
                self.instances.lock().await.insert(
                    entry.name.clone(),
                    Instance {
                        entry,
                        server,
                        address,
                    },
                );
            }
            Err(e) => log_error!("[{}] failed to start: {e}", entry.name),
        }
    }

    /// Re-read the registry and apply additions/removals.
    pub(crate) async fn reload(&self) -> Result<(), Error> {
        let registry = Registry::load(&self.global)?;
        self.apply_registry(&registry, true).await;
        Ok(())
    }

    /// Rebuild one tenant's manifest (called after `git publish`).
    pub(crate) async fn update_server(&self, key: &str) -> Result<(), Error> {
        let server = {
            let instances = self.instances.lock().await;
            instances
                .values()
                .find(|instance| instance.entry.matches(key))
                .map(|instance| Arc::clone(&instance.server))
        };
        let Some(server) = server else {
            return Err(Error::Daemon(format!("unknown server: {key}")));
        };
        server.update().await
    }

    pub(crate) async fn status_entries(&self) -> Vec<ipc::ServerStatus> {
        let mut entries: Vec<ipc::ServerStatus> = self
            .instances
            .lock()
            .await
            .values()
            .map(|instance| ipc::ServerStatus {
                name: instance.entry.name.clone(),
                path: instance.entry.path.display().to_string(),
                address: instance.address.clone(),
            })
            .collect();
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        entries
    }
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
            "[{}] Manifest ready: {} files, version {}",
            config.info.id,
            built.manifest.files.len(),
            built.manifest.version_hash
        );
        if let Some(signature) = &built.manifest.signature {
            log_info!(
                "[{}] Manifest signing enabled (ed25519 key fingerprint {})",
                config.info.id,
                signing::fingerprint(&signature.key)
            );
        } else if config.network.sign_manifest {
            log_warn!(
                "[{}] Manifest signing requested but no signature was produced",
                config.info.id
            );
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

    /// Rebuild the manifest from the current on-disk state.
    ///
    /// `git publish` commits before calling this, so deriving the changed files
    /// from `git status()` would always yield an empty list.
    pub(crate) async fn update(self: Arc<Self>) -> Result<(), Error> {
        self.rebuild_manifest().await?;
        let manifest = self.manifest.read().await.clone();
        log_info!(
            "[{}] Manifest updated ({} files, version {})",
            self.config.read().await.info.id,
            manifest.files.len(),
            manifest.version_hash
        );
        Ok(())
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
            log_warn!(
                "[{}] network/yggdrasil settings changed; restart the daemon to apply them",
                new_config.info.id
            );
        }

        log_info!("[{}] Config reloaded successfully", new_config.info.id);
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
