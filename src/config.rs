use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use syncraft_protocol::{Loader, VersionInfo};

#[derive(Debug, Serialize, Deserialize)]
pub struct SyncObject {
    pub path: PathBuf,
    pub to: PathBuf,
    pub is_mods_dir: Option<bool>
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    pub info: VersionInfo,
    pub sync: Vec<SyncObject>
}

impl Default for Config {
    fn default() -> Self {
        Self {
            info: VersionInfo {
                name: "<name>".into(),
                vanilla_version: "0.00.0".into(),
                loader: Loader::Vanilla
            },
            sync: Vec::new()
        }
    }
}