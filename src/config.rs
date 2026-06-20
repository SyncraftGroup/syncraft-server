use std::{fs, io::Write, path::PathBuf};

use serde::{Deserialize, Serialize};

use syncraft_protocol::{Loader, VersionInfo};

use crate::error::Error;

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

#[test]
fn test() -> Result<(), Error> {
    let config = Config {
        info: VersionInfo {
            name: "test".into(),
            vanilla_version: "test".into(),
            loader: Loader::Vanilla
        },
        sync: vec![SyncObject {
            path: PathBuf::from("foo"),
            to: PathBuf::from("bar"),
            is_mods_dir: None
        }]
    };
    let config_str = toml::to_string_pretty(&config).unwrap();
    let mut f = fs::File::create("test.toml")?;
    f.write(&config_str.into_bytes())?;
    Ok(())
}