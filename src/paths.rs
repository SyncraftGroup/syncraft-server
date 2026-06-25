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