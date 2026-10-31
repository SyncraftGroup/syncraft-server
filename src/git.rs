use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

use console_info::Ui;
use git2::{Delta, Oid, Repository, Sort};
use mod_type::SideStatus;
use syncraft_protocol::UpdateInfo;

use crate::daemon::ipc::{IpcRequest, IpcResponse, is_daemon_alive, send_request};
use crate::daemon::sync::{
    ModFileType, hash_file_blocking, load_mod_cache, mod_jars, save_mod_cache,
};
use crate::{config::Config, error::Error, log_debug, paths::SyncraftPaths};
use crate::{log_error, log_warn};

use crate::message::{MessageKey, translate};

pub enum GitAction {
    Init,
    Status,
    Publish,
}

pub fn run(paths: SyncraftPaths, action: Option<GitAction>) -> Result<(), Error> {
    let action = action.unwrap_or(GitAction::Status);

    let config = if let Some(config) = Config::load(&paths) {
        config
    } else {
        Ui::get().error(&translate(&MessageKey::Uninitialized));
        return Ok(());
    };
    match action {
        GitAction::Init => {
            init_repo(&paths, &config)?;
        }
        GitAction::Status => {
            let repository = if let Ok(repo) = GitRepository::open(&paths.root()) {
                repo
            } else {
                Ui::get().error(&translate(&MessageKey::Uninitialized));
                return Ok(());
            };
            generate_gitignore(&paths, &config)?;
            let changes = repository.status()?;
            Ui::get().echo(&translate(&MessageKey::Changes(changes)));
        }
        GitAction::Publish => {
            // Pre-check: daemon must be alive before going through the
            // editor / confirm / commit flow.
            if !is_daemon_alive(&paths.socket_path(), &paths.pid_file()) {
                Ui::get().error(&translate(&MessageKey::DaemonNotRunningPublish));
                return Ok(());
            }

            let repository = if let Ok(repo) = GitRepository::open(&paths.root()) {
                repo
            } else {
                Ui::get().error(&translate(&MessageKey::Uninitialized));
                return Ok(());
            };
            generate_gitignore(&paths, &config)?;
            let changes = repository.status()?;
            if changes.is_empty() {
                Ui::get().warn(&translate(&MessageKey::PublishAbortedEmptyUpdate));
                return Ok(());
            }

            let msg = CommitMsg::new(&paths);
            msg.create_default(&changes)?;

            let editor = get_editor();
            let edit_status = if let Ok(status) = msg.edit(&editor) {
                status
            } else {
                Ui::get().error(&translate(&MessageKey::FailedToLaunchEditor(editor)));
                return Ok(());
            };

            if !edit_status.success() {
                Ui::get().error(&translate(&MessageKey::EditorExitedUnexpectedly(
                    editor,
                    edit_status,
                )));
                return Ok(());
            }

            let msg_str = if let Some(msg_str) = msg.read()? {
                msg_str
            } else {
                // An empty message aborts the publish.
                Ui::get().warn(&translate(&MessageKey::PublishAbortedEmptyMessage));
                return Ok(());
            };

            let ui = Ui::get();
            ui.info(&translate(&MessageKey::Text(msg_str.clone())));
            ui.echo(&translate(&MessageKey::Changes(changes)));

            if !ui.confirm(&translate(&MessageKey::PublishConfirm))? {
                ui.warn(&translate(&MessageKey::PublishCancelled));
                return Ok(());
            }

            let oid = repository.commit(msg_str)?;

            // Apply the mod filter: analyse jars whose *content* is not in the
            // cache yet. This has to happen before the daemon rebuilds its
            // manifest, and only once: the manifest update is the last step.
            let mut mod_cache = load_mod_cache(&paths);
            let mut to_calculate = Vec::new();
            let mut current_hashes = HashSet::new();
            for jar in mod_jars(&config, &paths) {
                let hash = match hash_file_blocking(&jar) {
                    Ok(hash) => hash.to_hex().to_string(),
                    Err(e) => {
                        log_warn!("Could not hash {}: {e}", jar.display());
                        continue;
                    }
                };
                current_hashes.insert(hash.clone());
                match mod_cache.get(&hash) {
                    Some(ModFileType::Unknown) | None => to_calculate.push((jar, hash)),
                    _ => {}
                }
            }

            if !to_calculate.is_empty() {
                if to_calculate.len() > 2 {
                    ui.step(&translate(&MessageKey::ModFilterMayTakeAWhile));
                }
                for (jar, hash) in to_calculate {
                    let relative = jar.strip_prefix(paths.root()).unwrap_or(&jar).to_path_buf();

                    match mod_type::analyze_mod_jar(&jar) {
                        Ok(info) => {
                            let mod_type = match &info.client {
                                &SideStatus::Required => ModFileType::Sync,
                                &SideStatus::Optional => ModFileType::Sync,
                                &SideStatus::Unsupported => ModFileType::Ignore,
                            };
                            ui.ok(&translate(&MessageKey::GotModType(&relative, &mod_type)));
                            mod_cache.insert(hash, mod_type);
                        }
                        Err(e) => {
                            log_warn!("Mod lookup failed for {}: {e}", jar.display());
                            let option_ignore = translate(&MessageKey::SelectOptionIgnore);
                            let option_sync = translate(&MessageKey::SelectOptionSync);

                            let selected = ui.select(
                                &translate(&MessageKey::SelectModType(&relative)),
                                vec![&option_ignore, &option_sync],
                            )?;

                            let mod_type = if selected == &option_ignore {
                                ModFileType::Ignore
                            } else {
                                ModFileType::Sync
                            };
                            ui.ok(&translate(&MessageKey::GotModType(&relative, &mod_type)));
                            mod_cache.insert(hash, mod_type);
                        }
                    }
                }
            }

            // Drop decisions for jars that are no longer installed.
            mod_cache.retain(|hash, _| current_hashes.contains(hash));
            save_mod_cache(&paths, &mod_cache)?;

            publish_request(oid, &paths)?;
        }
    }
    Ok(())
}

fn publish_request(oid: Oid, paths: &SyncraftPaths) -> Result<(), Error> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| Error::Daemon(format!("Failed to create runtime: {}", e)))?;

    match rt.block_on(send_request(&paths.socket_path(), &IpcRequest::Update)) {
        Ok(IpcResponse::Ok) => {
            let hash = oid.to_string();
            Ui::get().ok(&translate(&MessageKey::Published(hash)));
        }
        Ok(IpcResponse::Error { message }) => {
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
}

#[derive(Debug)]
pub enum Change {
    Added(PathBuf),
    Modified(PathBuf),
    Deleted(PathBuf),
    Renamed { from: PathBuf, to: PathBuf },
    Ignored,
}

pub struct GitRepository {
    inner: Repository,
}

impl GitRepository {
    fn init(path: &Path) -> Result<Self, Error> {
        Ok(Self {
            inner: Repository::init(path)?,
        })
    }

    pub fn open(path: &Path) -> Result<Self, Error> {
        Ok(Self {
            inner: Repository::open(path)?,
        })
    }

    fn commit(&self, msg: String) -> Result<Oid, Error> {
        let mut index = self.inner.index()?;
        index.add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)?;
        index.write()?;

        let tree_id = index.write_tree()?;
        let tree = self.inner.find_tree(tree_id)?;

        let sig = git2::Signature::now("syncraft", "syncraft@local")?;

        let parent = self
            .inner
            .head()
            .ok()
            .and_then(|h| h.target())
            .and_then(|id| self.inner.find_commit(id).ok());

        let oid = match parent {
            Some(parent) => self
                .inner
                .commit(Some("HEAD"), &sig, &sig, &msg, &tree, &[&parent])?,
            None => self
                .inner
                .commit(Some("HEAD"), &sig, &sig, &msg, &tree, &[])?,
        };
        Ok(oid)
    }

    pub fn status(&self) -> Result<Vec<Change>, Error> {
        let head = self.inner.head()?.peel_to_tree()?;
        let diff = self.inner.diff_tree_to_workdir_with_index(
            Some(&head),
            Some(git2::DiffOptions::new().include_untracked(true)),
        )?;

        let err = || git2::Error::from_str("Invalid delta info");

        let mut changes = Vec::new();
        for delta in diff.deltas() {
            let status = delta.status();
            let change = match &status {
                Delta::Added | Delta::Untracked => Ok(Change::Added(PathBuf::from(
                    delta.new_file().path().ok_or_else(err)?,
                ))),
                Delta::Modified => Ok(Change::Modified(PathBuf::from(
                    delta.new_file().path().ok_or_else(err)?,
                ))),
                Delta::Deleted => Ok(Change::Deleted(PathBuf::from(
                    delta.old_file().path().ok_or_else(err)?,
                ))),
                Delta::Renamed => Ok(Change::Renamed {
                    from: PathBuf::from(delta.old_file().path().ok_or_else(err)?),
                    to: PathBuf::from(delta.new_file().path().ok_or_else(err)?),
                }),
                Delta::Ignored | Delta::Unmodified => Ok(Change::Ignored),
                _ => Err(Error::Git(git2::Error::from_str("Unexpected delta type"))),
            }?;
            changes.push(change);
        }
        Ok(changes)
    }

    pub fn latest_hash(&self) -> Result<Oid, Error> {
        self.inner
            .head()?
            .target()
            .ok_or_else(|| Error::Git(git2::Error::from_str("Failed to get latest hash")))
    }

    /// Commits between HEAD and `since` (exclusive), newest first.
    ///
    /// Returns `Ok(None)` when `since` is not part of this history, so callers
    /// can answer 404 instead of dumping every commit in the repository.
    pub fn commits_after(&self, since: &str) -> Result<Option<Vec<UpdateInfo>>, git2::Error> {
        const MAX_COMMITS: usize = 100;

        let since = git2::Oid::from_str(since.trim())
            .map_err(|_| git2::Error::from_str("invalid commit hash"))?;

        let mut walk = self.inner.revwalk()?;
        walk.push_head()?;
        walk.set_sorting(Sort::TOPOLOGICAL | Sort::TIME)?;

        let mut messages = Vec::new();
        let mut found = false;

        for oid in walk {
            let oid = oid?;
            if oid == since {
                found = true;
                break;
            }
            if messages.len() >= MAX_COMMITS {
                break;
            }
            let commit = self.inner.find_commit(oid)?;
            messages.push(UpdateInfo {
                message: commit.summary().ok().flatten().unwrap_or("").to_string(),
                hash: oid.to_string(),
            });
        }

        Ok(found.then_some(messages))
    }
}

fn init_repo(paths: &SyncraftPaths, config: &Config) -> Result<(), Error> {
    let repo = GitRepository::init(&paths.root())?;
    generate_gitignore(paths, config)?;
    repo.commit("Initialize workspace".into())?;
    log_debug!("Git repository initialised");
    Ok(())
}

/// Generate and write a .gitignore file from `sync` in config.
fn generate_gitignore(paths: &SyncraftPaths, config: &Config) -> Result<(), Error> {
    let mut gitignore = default_gitignore();
    for object in &config.sync {
        mark_in_gitignore(&mut gitignore, &object.source);
        if object.source.is_dir() {
            gitignore.push(format!("!{}/**", object.source.to_string_lossy()));
        }
    }
    std::fs::write(paths.gitignore(), gitignore.join("\n"))?;
    Ok(())
}

fn default_gitignore() -> Vec<String> {
    vec![
        "*".to_string(),
        "!.gitignore".to_string(),
        "!syncraft.toml".to_string(),
    ]
}

/// Recursively add `!path` entries so git tracks the given path.
fn mark_in_gitignore(gitignore: &mut Vec<String>, path: &Path) {
    if let Some(parent) = path.parent()
        && parent != Path::new("")
        && !gitignore.contains(&format!("!{}", parent.to_string_lossy()))
    {
        mark_in_gitignore(gitignore, parent);
    }
    if path.is_dir() {
        gitignore.push(format!("!{}/", path.to_string_lossy()));
    } else {
        gitignore.push(format!("!{}", path.to_string_lossy()));
    }
}

pub struct CommitMsg {
    file_path: PathBuf,
}

const DEFAULT_EDITOR: &str = "vi";
fn get_editor() -> String {
    std::env::var("SYNCRAFT_EDITOR").unwrap_or_else(|_| DEFAULT_EDITOR.to_string())
}

impl CommitMsg {
    fn new(paths: &SyncraftPaths) -> Self {
        Self {
            file_path: paths.commit_msg(),
        }
    }

    fn create_default(&self, changes: &Vec<Change>) -> Result<(), Error> {
        let mut f = fs::File::create(&self.file_path)?;
        writeln!(f, "# Server update message.")?;
        writeln!(f, "# ")?;
        writeln!(f, "# Changes to be committed:")?;
        for change in changes {
            match &change {
                Change::Added(path) => {
                    writeln!(f, "#   {:>8}: {}", "new file", path.to_string_lossy())?
                }
                Change::Deleted(path) => {
                    writeln!(f, "#   {:>8}: {}", "deleted", path.to_string_lossy())?
                }
                Change::Modified(path) => {
                    writeln!(f, "#   {:>8}: {}", "modified", path.to_string_lossy())?
                }
                Change::Renamed { from, to } => writeln!(
                    f,
                    "#   {:>8}: {} -> {}",
                    "renamed",
                    from.to_string_lossy(),
                    to.to_string_lossy()
                )?,
                Change::Ignored => (),
            };
        }
        writeln!(f, "# ")?;
        writeln!(f, "# Please enter your update message under.")?;
        writeln!(f, "# ")?;
        writeln!(f, "# Lines starting with `#` will be ignored.")?;
        writeln!(f, "# An empty message aborts the operation.")?;
        writeln!(f, "# ")?;

        Ok(())
    }

    fn edit(&self, editor: &str) -> Result<ExitStatus, Error> {
        let status = Command::new(editor).arg(&self.file_path).status()?;
        Ok(status)
    }

    fn read(&self) -> Result<Option<String>, Error> {
        let msg_lines: Vec<String> = fs::read_to_string(&self.file_path)?
            .lines()
            .filter_map(|line| {
                if line.starts_with("#") {
                    None
                } else {
                    Some(line.to_string())
                }
            })
            .collect();

        let msg = msg_lines.join("\n").trim().to_string();

        if msg.is_empty() {
            Ok(None)
        } else {
            Ok(Some(msg))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_repo(name: &str) -> (PathBuf, GitRepository) {
        let root = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let repo = GitRepository::init(&root).unwrap();
        (root, repo)
    }

    #[test]
    fn commits_after_only_walks_the_requested_range() {
        let (root, repo) = temp_repo("syncraft_test_commits_after");
        std::fs::write(root.join("a.txt"), b"a").unwrap();
        let first = repo.commit("first".into()).unwrap();
        std::fs::write(root.join("b.txt"), b"b").unwrap();
        let second = repo.commit("second".into()).unwrap();

        let updates = repo.commits_after(&first.to_string()).unwrap().unwrap();
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].hash, second.to_string());
        assert_eq!(updates[0].message, "second");

        // Nothing after HEAD.
        assert_eq!(
            repo.commits_after(&second.to_string())
                .unwrap()
                .unwrap()
                .len(),
            0
        );

        // Unknown but well-formed commit → None instead of the whole history.
        assert!(repo.commits_after(&"0".repeat(40)).unwrap().is_none());
        // Malformed hash → error (the HTTP layer answers 404/500).
        assert!(repo.commits_after("not-a-hash").is_err());

        let _ = std::fs::remove_dir_all(root);
    }
}
