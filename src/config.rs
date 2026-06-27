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
        let config: Config = toml::from_str(&config_str).ok()?;

        Some(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tokio::fs;

    #[test]
    fn config_default_values() {
        let config = Config::default();
        assert_eq!(config.info.name, "<name>");
        assert_eq!(config.info.vanilla_version, "0.00.0");
        assert!(config.sync.is_empty());
    }

    #[test]
    fn sync_object_defaults() {
        let obj = SyncObject {
            path: PathBuf::from("mods"),
            to: PathBuf::from("mods"),
            is_mods_dir: None,
        };
        assert!(obj.is_mods_dir.is_none());
    }

    #[tokio::test]
    async fn config_load_no_workspace_returns_none() {
        let paths = SyncraftPaths::new(PathBuf::from("/tmp/no-such-dir-12345"));
        let config = Config::load(&paths).await;
        assert!(config.is_none());
    }

    #[tokio::test]
    async fn config_load_valid_file() {
        let tmp = std::env::temp_dir().join("syncraft_test_config");
        let _ = fs::create_dir_all(tmp.join("syncraft")).await;
        let paths = SyncraftPaths::new(tmp.clone());

        let toml_content = r#"
[info]
name = "TestServer"
vanilla_version = "1.20.1"
loader = "Vanilla"

[[sync]]
path = "mods"
to = "mods"

[yggdrasil]
base_url = "http://localhost/yggdrasil"
skin_domains = ["example.com"]
"#;
        fs::write(paths.config_path(), toml_content).await.unwrap();

        let config = Config::load(&paths).await.unwrap();
        assert_eq!(config.info.name, "TestServer");
        assert_eq!(config.info.vanilla_version, "1.20.1");
        assert_eq!(config.sync.len(), 1);
        assert_eq!(config.yggdrasil.base_url, "http://localhost/yggdrasil");
        assert_eq!(config.yggdrasil.skin_domains, vec!["example.com"]);

        // Cleanup
        let _ = fs::remove_dir_all(tmp.join("syncraft")).await;
        let _ = fs::remove_file(tmp.join("syncraft.toml")).await;
    }

    #[tokio::test]
    async fn config_load_invalid_toml() {
        let tmp = std::env::temp_dir().join("syncraft_test_invalid");
        let _ = fs::create_dir_all(tmp.join("syncraft")).await;
        let paths = SyncraftPaths::new(tmp.clone());

        fs::write(paths.config_path(), "not valid toml {{{").await.unwrap();

        let config = Config::load(&paths).await;
        assert!(config.is_none());

        // Cleanup
        let _ = fs::remove_dir_all(tmp.join("syncraft")).await;
        let _ = fs::remove_file(tmp.join("syncraft.toml")).await;
    }
}