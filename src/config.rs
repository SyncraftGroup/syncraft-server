use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use syncraft_protocol::{Loader, VersionInfo};

use crate::{paths::SyncraftPaths, yggdrasil::YggdrasilConfig};
use crate::log_debug;
use crate::log_warn;

#[derive(Debug, Serialize, Deserialize)]
pub struct SyncObject {
    pub path: PathBuf,
    pub to: PathBuf,
    pub is_mods_dir: Option<bool>
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NetworkConfig {
    pub host: String,
    pub port: u16
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    pub info: VersionInfo,
    pub network: NetworkConfig,
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
            network: NetworkConfig {
                host: "127.0.0.1".into(),
                port: 3000,
            },
            sync: Vec::new(),
            yggdrasil: YggdrasilConfig::default()
        }
    }
}

impl Config {
    pub fn load(paths: &SyncraftPaths) -> Option<Self>  {
        if !paths.workspace().exists() {
            log_debug!("Workspace not found at {}", paths.workspace().display());
            return None;
        }

        let config_path = paths.config_path();
        let config_str = match std::fs::read_to_string(&config_path) {
            Ok(s) => s,
            Err(e) => {
                log_warn!("Failed to read config at {}: {e}", config_path.display());
                return None;
            }
        };
        let config: Config = match toml::from_str(&config_str) {
            Ok(c) => c,
            Err(e) => {
                log_warn!("Failed to parse config at {}: {e}", config_path.display());
                return None;
            }
        };

        log_debug!(
            "Config loaded: {} (MC {})",
            config.info.name,
            config.info.vanilla_version
        );
        Some(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::fs;

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
        let config = Config::load(&paths);
        assert!(config.is_none());
    }

    #[tokio::test]
    async fn config_load_valid_file() {
        let tmp = std::env::temp_dir().join("syncraft_test_config");
        let _ = fs::create_dir_all(tmp.join("syncraft"));
        let paths = SyncraftPaths::new(tmp.clone());

        let toml_content = r#"
[info]
name = "TestServer"
vanilla_version = "1.20.1"
loader = "Vanilla"

[network]
host = "127.0.0.1"
port = 3000

[[sync]]
path = "mods"
to = "mods"

[yggdrasil]
base_url = "http://localhost/yggdrasil"
skin_domains = ["example.com"]
identity_falsification = false
"#;
        fs::write(paths.config_path(), toml_content).unwrap();

        let config = Config::load(&paths).unwrap();
        assert_eq!(config.info.name, "TestServer");
        assert_eq!(config.info.vanilla_version, "1.20.1");
        assert_eq!(config.sync.len(), 1);
        assert_eq!(config.yggdrasil.base_url, "http://localhost/yggdrasil");
        assert_eq!(config.yggdrasil.skin_domains, vec!["example.com"]);

        // Cleanup
        let _ = fs::remove_dir_all(tmp.join("syncraft"));
        let _ = fs::remove_file(tmp.join("syncraft.toml"));
    }

    #[tokio::test]
    async fn config_load_invalid_toml() {
        let tmp = std::env::temp_dir().join("syncraft_test_invalid");
        let _ = fs::create_dir_all(tmp.join("syncraft"));
        let paths = SyncraftPaths::new(tmp.clone());

        fs::write(paths.config_path(), "not valid toml {{{").unwrap();

        let config = Config::load(&paths);
        assert!(config.is_none());

        // Cleanup
        let _ = fs::remove_dir_all(tmp.join("syncraft"));
        let _ = fs::remove_file(tmp.join("syncraft.toml"));
    }
}