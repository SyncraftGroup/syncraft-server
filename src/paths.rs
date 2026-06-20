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
            fs::create_dir_all(self.data())
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

    pub fn data(&self) -> PathBuf {
        self.workspace().join("data")
    }
}