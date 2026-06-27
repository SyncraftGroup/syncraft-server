use std::path::PathBuf;

use tokio::fs;

use crate::error::Error;

pub struct SyncraftPaths {
    root: PathBuf
}

impl SyncraftPaths {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub async fn create_dirs(&self) -> Result<(), Error> {
        tokio::try_join!(
            fs::create_dir_all(self.root()),
            fs::create_dir_all(self.workspace()),
            fs::create_dir_all(self.data()),
            fs::create_dir_all(self.yggdrasil().root())
        )?;
        Ok(())
    }

    pub fn root(&self) -> PathBuf {
        self.root.clone()
    }

    pub fn workspace(&self) -> PathBuf {
        self.root().join("syncraft")
    }

    pub fn config_path(&self) -> PathBuf {
        self.root().join("syncraft.toml")
    }

    pub fn yggdrasil(&self) -> YggdrasilPaths<'_> {
        YggdrasilPaths { paths: self }
    }

    pub fn data(&self) -> PathBuf {
        self.workspace().join("data")
    }
}

pub struct YggdrasilPaths<'a> {
    pub paths: &'a SyncraftPaths
}

impl YggdrasilPaths<'_> {

    pub fn root(&self) -> PathBuf {
        self.paths.root().join("yggdrasil")
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
        assert_eq!(paths.workspace(), PathBuf::from("/home/user/mcserver/syncraft"));
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
        assert_eq!(yp.root(), PathBuf::from("/mc/yggdrasil"));
    }

    #[test]
    fn yggdrasil_paths_db() {
        let paths = SyncraftPaths::new(PathBuf::from("/mc"));
        let yp = paths.yggdrasil();
        assert_eq!(yp.db_path(), PathBuf::from("/mc/yggdrasil/users.db"));
    }

    #[test]
    fn yggdrasil_paths_private_key() {
        let paths = SyncraftPaths::new(PathBuf::from("/mc"));
        let yp = paths.yggdrasil();
        assert_eq!(yp.private_key(), PathBuf::from("/mc/yggdrasil/key.pem"));
    }
}