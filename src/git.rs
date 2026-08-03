use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

use console_info::Ui;
use git2::{Delta, Oid, Repository, Sort};
use syncraft_protocol::UpdateInfo;

use crate::daemon::ipc::{IpcRequest, IpcResponse, is_daemon_alive, send_request};
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

    pub fn commits_after(&self, since: &str) -> Result<Vec<UpdateInfo>, git2::Error> {
        let mut walk = self.inner.revwalk()?;
        walk.push_head()?;
        walk.set_sorting(Sort::TOPOLOGICAL | Sort::TIME)?;

        let mut messages = Vec::new();

        for oid in walk {
            let oid = oid?;
            let oid_str = oid.to_string();

            if oid_str == since {
                break;
            }

            let commit = self.inner.find_commit(oid)?;
            messages.push(UpdateInfo {
                message: commit.summary()?.unwrap_or("").to_string(),
                hash: oid_str,
            });
        }

        Ok(messages)
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
