use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use syncraft_protocol::{Loader, VersionInfo};
use tokio::fs;

use crate::{paths::SyncraftPaths, yggdrasil::YggdrasilConfig};

#[derive(Debug, Serialize, Deserialize)]
pub struct SyncObject {
    pub path: PathBuf,
    pub to: PathBuf,
    pub is_mods_dir: Option<bool>
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    pub info: VersionInfo,
    pub sync: Vec<SyncObject>,
    pub yggdrasil: YggdrasilConfig
}

impl Default for Config {
    fn default() -> Self {
        Self {
            info: VersionInfo {
                name: "<name>".into(),
                vanilla_version: "0.00.0".into(),
                loader: Loader::Vanilla
            },
            sync: Vec::new(),
            yggdrasil: YggdrasilConfig::default()
        }
    }
}

impl Config {
    pub async fn load(paths: &SyncraftPaths) -> Option<Self>  {
        if !paths.workspace().exists() {
            return None;
        }

        let config_str = fs::read_to_string(paths.config_path()).await.ok()?;
        let config: Config = serde_json::from_str(&config_str).ok()?;

        Some(config)
    }
}