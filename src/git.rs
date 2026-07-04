use std::path::Path;

use git2::{Delta, Repository};

use crate::{config::Config, error::Error, log_debug, paths::SyncraftPaths, ui::{Ui, message::{MessageKey, translate}}};


pub enum GitAction {
    /// Initialize a repository.
    Init,
    /// Get current tracking status.
    Status
}

/// Main entrance of git actions.
pub async fn run(paths: &SyncraftPaths, action: Option<GitAction>) -> Result<(), Error> {

    let action = action.unwrap_or(GitAction::Status);

    let config = 
    if let Some(config) = Config::load(paths) { config }
    else {
        Ui::get().error(MessageKey::Uninitialized);
        return Ok(());
    };
    match action {
        GitAction::Init => {
            init_repo(paths, &config).await?;
        }
        GitAction::Status => {
            let repository =
            if let Ok(repo) = GitRepository::open(&paths.root()) { repo }
            else {
                Ui::get().error(MessageKey::Uninitialized);
                return Ok(());
            };
            generate_gitignore(paths, &config).await?;
            let changes = repository.status()?;
            Ui::get().echo(&translate(&MessageKey::Changes(changes)));
        }
    }
    Ok(())
}

#[derive(Debug)]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed,
    Ignored
}

impl TryFrom<git2::Delta> for ChangeKind {
    type Error = Error;
    fn try_from(value: git2::Delta) -> Result<Self, Self::Error> {
        match value {
            Delta::Added | Delta::Untracked => Ok(Self::Added),
            Delta::Modified | Delta::Copied | Delta::Typechange => Ok(Self::Modified),
            Delta::Deleted => Ok(Self::Deleted),
            Delta::Renamed => Ok(Self::Renamed),
            Delta::Ignored | Delta::Unmodified => Ok(Self::Ignored),
            _ => Err(Error::Git(git2::Error::from_str("Unexpected delta type")))
        }
    }
}

#[derive(Debug)]
pub struct Change {
    pub kind: ChangeKind,
    pub path: String
}

struct GitRepository {
    inner: Repository
}

impl GitRepository {

    fn init(path: &Path) -> Result<Self, Error> {
        Ok(Self { inner: Repository::init(path)? })
    }

    fn open(path: &Path) -> Result<Self, Error> {
        Ok(Self { inner: Repository::open(path)? })
    }

    /// Stage all delta & commit to a repository.
    fn commit(&self, msg: String) -> Result<(), Error> {
        let mut index = self.inner.index()?;
        index.add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)?;
        index.write()?;

        let tree_id = index.write_tree()?;
        let tree = self.inner.find_tree(tree_id)?;

        let sig = git2::Signature::now("syncraft", "syncraft@local")?;
        
        let parent = self.inner.head()
            .ok()
            .and_then(|h| h.target())
            .and_then(|id| self.inner.find_commit(id).ok());

        match parent {
            Some(parent) => {
                self.inner.commit(
                    Some("HEAD"),
                    &sig,
                    &sig,
                    &msg,
                    &tree,
                    &[&parent],
                )?;
            }
            None => {
                // init commit
                self.inner.commit(
                    Some("HEAD"),
                    &sig,
                    &sig,
                    &msg,
                    &tree,
                    &[],
                )?;
            }
        }
        Ok(())
    }

    fn status(&self) -> Result<Vec<Change>, Error> {
        let head = self.inner.head()?.peel_to_tree()?;
        let diff = self.inner.diff_tree_to_workdir_with_index(
            Some(&head),
            Some(git2::DiffOptions::new().include_untracked(true)),
        )?;

        let mut changes = Vec::new();
        for delta in diff.deltas() {
            let status = delta.status();
            let path = match &status {
                Delta::Deleted => delta.old_file(),
                _ => delta.new_file()
            }.path();
            changes.push(Change {
                kind: ChangeKind::try_from(status)?,
                path: path.map_or("???".to_string(), |path| {
                    path.to_string_lossy().into_owned()
                })
            });
        }
        Ok(changes)
    }
}

/// Initialize a repository.
async fn init_repo(paths: &SyncraftPaths, config: &Config) -> Result<(), Error> {
    let repo = GitRepository::init(&paths.root())?;
    generate_gitignore(paths, config).await?;
    repo.commit("Initialize workspace".into())?;
    log_debug!("Git repository initialised");
    Ok(())
}

/// Generate and write a .gitignore file.
/// The contents are generated from `sync` in config.
async fn generate_gitignore(paths: &SyncraftPaths, config: &Config) -> Result<(), Error> {
    let mut gitignore = default_gitignore();
    for object in &config.sync {
        mark_in_gitignore(&mut gitignore, &object.path);
        if object.path.is_dir() {
            gitignore.push(format!("!{}/**", object.path.to_string_lossy()));
        }
    }
    tokio::fs::write(paths.gitignore(), gitignore.join("\n")).await?;
    Ok(())
}

/// Get a default .gitignore vec.
fn default_gitignore() -> Vec<String> {
    vec![
        "*".to_string(),
        "!.gitignore".to_string(),
        "!syncraft.toml".to_string(),
        "!syncraft/".to_string(),
        "!syncraft/**".to_string(),
        "syncraft/logs/".to_string(),
        "syncraft/logs/**".to_string()
    ]
}

/// Recursively push !ignore to the vec.
fn mark_in_gitignore(gitignore: &mut Vec<String>, path: &Path) {
    if let Some(parent) = path.parent()
    && parent != Path::new("")
    && !gitignore.contains(&format!("!{}", parent.to_string_lossy())) {
        mark_in_gitignore(gitignore, parent);
    }
    if path.is_dir() {
        gitignore.push(format!("!{}/", path.to_string_lossy()));
    } else {
        gitignore.push(format!("!{}", path.to_string_lossy()));
    }
}