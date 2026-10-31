use std::path::PathBuf;

use chrono::Local;
use std::fs;

use crate::error::Error;

/// Per-user daemon state, shared by every registered Minecraft server.
///
/// The daemon is no longer bound to one server directory: it keeps its registry,
/// socket, PID and logs here, while each registered server keeps its own
/// workspace (config, manifests, accounts, email template) under its own root.
#[derive(Debug, Clone)]
pub struct GlobalPaths {
    root: PathBuf,
}

impl GlobalPaths {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// `$XDG_DATA_HOME/syncraft`, falling back to `~/.local/share/syncraft`.
    ///
    /// `SYNCRAFT_DATA_DIR` overrides the location (the generated systemd unit
    /// pins it so the unit always uses the same registry as `daemon install`).
    pub fn discover() -> Self {
        if let Some(root) = std::env::var_os("SYNCRAFT_DATA_DIR")
            .map(PathBuf::from)
            .filter(|path| !path.as_os_str().is_empty())
        {
            return Self::new(root);
        }

        let root = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|path| !path.as_os_str().is_empty())
            .map(|path| path.join("syncraft"))
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .map(|home| home.join(".local").join("share").join("syncraft"))
            })
            .unwrap_or_else(|| std::env::temp_dir().join("syncraft"));
        Self::new(root)
    }

    pub fn root(&self) -> PathBuf {
        self.root.clone()
    }

    pub fn create_dirs(&self) -> Result<(), Error> {
        fs::create_dir_all(self.logs_dir())?;
        Ok(())
    }

    /// TOML list of registered server roots.
    pub fn registry_path(&self) -> PathBuf {
        self.root.join("servers.toml")
    }

    pub fn socket_path(&self) -> PathBuf {
        self.root.join("daemon.sock")
    }

    pub fn pid_file(&self) -> PathBuf {
        self.root.join("daemon.pid")
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }

    pub fn log_file_latest(&self) -> PathBuf {
        self.logs_dir().join("latest.log")
    }

    pub fn log_file(&self) -> PathBuf {
        let timestamp = Local::now().format("%Y-%m-%d-%H-%M-%S");
        self.logs_dir().join(format!("{timestamp}.log"))
    }
}

#[derive(Debug, Clone)]
pub struct SyncraftPaths {
    root: PathBuf,
}

impl SyncraftPaths {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn create_dirs(&self) -> Result<(), Error> {
        fs::create_dir_all(self.root())?;
        fs::create_dir_all(self.workspace())?;
        fs::create_dir_all(self.data())?;
        fs::create_dir_all(self.yggdrasil().root())?;
        Ok(())
    }

    pub fn root(&self) -> PathBuf {
        self.root.clone()
    }

    /// Resolve a config-relative path against the workspace root.
    ///
    /// Config paths are documented as relative to the Minecraft server
    /// directory; the daemon does not depend on the process working directory.
    pub fn resolve(&self, path: &std::path::Path) -> PathBuf {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.root().join(path)
        }
    }

    pub fn workspace(&self) -> PathBuf {
        self.root().join("syncraft")
    }

    pub fn config_path(&self) -> PathBuf {
        self.root().join("syncraft.toml")
    }

    /// Optional HTML template overriding the verification email body.
    /// Placed next to `syncraft.toml`.
    pub fn email_template(&self) -> PathBuf {
        self.root().join(crate::yggdrasil::email::TEMPLATE_FILE)
    }

    pub fn gitignore(&self) -> PathBuf {
        self.root().join(".gitignore")
    }

    pub fn commit_msg(&self) -> PathBuf {
        self.workspace().join("COMMIT_EDITMSG")
    }

    pub fn yggdrasil(&self) -> YggdrasilPaths<'_> {
        YggdrasilPaths { paths: self }
    }

    pub fn data(&self) -> PathBuf {
        self.workspace().join("data")
    }

    pub fn mod_type_cache(&self) -> PathBuf {
        self.data().join("mod_type_cache.json")
    }

    pub fn hash_cache(&self) -> PathBuf {
        self.data().join("hash_cache.json")
    }

    pub fn manifests_dir(&self) -> PathBuf {
        self.data().join("manifests")
    }
}

pub struct YggdrasilPaths<'a> {
    pub paths: &'a SyncraftPaths,
}

impl YggdrasilPaths<'_> {
    pub fn root(&self) -> PathBuf {
        self.paths.workspace().join("yggdrasil")
    }

    pub fn db_path(&self) -> PathBuf {
        self.root().join("users.db")
    }

    pub fn private_key(&self) -> PathBuf {
        self.root().join("key.pem")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn syncraft_paths_root() {
        let paths = SyncraftPaths::new(PathBuf::from("/home/user/mcserver"));
        assert_eq!(paths.root(), PathBuf::from("/home/user/mcserver"));
    }

    #[test]
    fn syncraft_paths_workspace() {
        let paths = SyncraftPaths::new(PathBuf::from("/home/user/mcserver"));
        assert_eq!(
            paths.workspace(),
            PathBuf::from("/home/user/mcserver/syncraft")
        );
    }

    #[test]
    fn syncraft_paths_config() {
        let paths = SyncraftPaths::new(PathBuf::from("/home/user/mcserver"));
        assert_eq!(
            paths.config_path(),
            PathBuf::from("/home/user/mcserver/syncraft.toml")
        );
    }

    #[test]
    fn syncraft_paths_email_template() {
        let paths = SyncraftPaths::new(PathBuf::from("/home/user/mcserver"));
        assert_eq!(
            paths.email_template(),
            PathBuf::from("/home/user/mcserver/verify-email.html")
        );
    }

    #[test]
    fn syncraft_paths_data() {
        let paths = SyncraftPaths::new(PathBuf::from("/home/user/mcserver"));
        assert_eq!(
            paths.data(),
            PathBuf::from("/home/user/mcserver/syncraft/data")
        );
    }

    #[test]
    fn yggdrasil_paths_root() {
        let paths = SyncraftPaths::new(PathBuf::from("/mc"));
        let yp = paths.yggdrasil();
        assert_eq!(yp.root(), PathBuf::from("/mc/syncraft/yggdrasil"));
    }

    #[test]
    fn yggdrasil_paths_db() {
        let paths = SyncraftPaths::new(PathBuf::from("/mc"));
        let yp = paths.yggdrasil();
        assert_eq!(
            yp.db_path(),
            PathBuf::from("/mc/syncraft/yggdrasil/users.db")
        );
    }

    #[test]
    fn global_paths_layout() {
        let global = GlobalPaths::new(PathBuf::from("/home/u/.local/share/syncraft"));
        assert_eq!(
            global.registry_path(),
            PathBuf::from("/home/u/.local/share/syncraft/servers.toml")
        );
        assert_eq!(
            global.socket_path(),
            PathBuf::from("/home/u/.local/share/syncraft/daemon.sock")
        );
        assert_eq!(
            global.pid_file(),
            PathBuf::from("/home/u/.local/share/syncraft/daemon.pid")
        );
        assert_eq!(
            global.log_file_latest(),
            PathBuf::from("/home/u/.local/share/syncraft/logs/latest.log")
        );
    }

    #[test]
    fn yggdrasil_paths_private_key() {
        let paths = SyncraftPaths::new(PathBuf::from("/mc"));
        let yp = paths.yggdrasil();
        assert_eq!(
            yp.private_key(),
            PathBuf::from("/mc/syncraft/yggdrasil/key.pem")
        );
    }
}
