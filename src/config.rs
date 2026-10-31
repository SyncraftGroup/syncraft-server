use std::path::PathBuf;

use path_clean::PathClean;
use serde::{Deserialize, Deserializer, Serialize};

use syncraft_protocol::{Loader, SyncMode, VersionInfo};

use crate::log_debug;
use crate::log_warn;
use crate::{paths::SyncraftPaths, yggdrasil::YggdrasilConfig};

fn deserialize_clean_path<'de, D>(deserializer: D) -> Result<PathBuf, D::Error>
where
    D: Deserializer<'de>,
{
    let path = PathBuf::deserialize(deserializer)?;
    Ok(path.clean())
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SyncObject {
    #[serde(deserialize_with = "deserialize_clean_path")]
    pub source: PathBuf,
    #[serde(deserialize_with = "deserialize_clean_path")]
    pub target: PathBuf,
    #[serde(default)]
    pub mode: SyncMode,
    #[serde(default)]
    pub is_mod_dir: bool,
    /// Explicitly treat `target` as a directory when `source` is a single file.
    ///
    /// By default this is guessed from the server-side path, which is ambiguous
    /// for names without an extension; set it when the guess is wrong.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_is_dir: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct TlsConfig {
    /// PEM certificate chain.
    pub cert: PathBuf,
    /// PEM private key.
    pub key: PathBuf,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct NetworkConfig {
    pub host: String,
    pub port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modrinth_api: Option<String>,
    /// Optional HTTPS listener. Plain HTTP remains the default: most private
    /// servers are on a LAN or behind a TLS-terminating proxy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls: Option<TlsConfig>,
    /// Sign the manifest with the server key. Recommended when serving HTTP.
    #[serde(default)]
    pub sign_manifest: bool,
}

/// Tunables for how the server stores and serves versions.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ServerSettings {
    /// How many manifest snapshots to keep on disk. `0` keeps all of them.
    #[serde(default = "default_snapshot_retention")]
    pub snapshot_retention: usize,
    /// Suggested parallel downloads for clients (1..=16).
    #[serde(default = "default_download_concurrency")]
    pub download_concurrency: u8,
}

fn default_snapshot_retention() -> usize {
    32
}

fn default_download_concurrency() -> u8 {
    4
}

impl Default for ServerSettings {
    fn default() -> Self {
        Self {
            snapshot_retention: default_snapshot_retention(),
            download_concurrency: default_download_concurrency(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Config {
    pub info: VersionInfo,
    pub network: NetworkConfig,
    #[serde(default)]
    pub server: ServerSettings,
    pub sync: Vec<SyncObject>,
    /// Client paths that must not exist. Unlike `sync[].mode = "forbidden"`,
    /// these do not require a matching file on the server.
    #[serde(default)]
    pub forbid: Vec<PathBuf>,
    pub yggdrasil: YggdrasilConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            info: VersionInfo {
                name: "<name>".into(),
                id: "syncraft".into(),
                vanilla_version: "0.00.0".into(),
                loader: Loader::Vanilla,
            },
            network: NetworkConfig {
                host: "127.0.0.1".into(),
                port: 3000,
                modrinth_api: None,
                tls: None,
                sign_manifest: false,
            },
            server: ServerSettings::default(),
            sync: Vec::new(),
            forbid: Vec::new(),
            yggdrasil: YggdrasilConfig::default(),
        }
    }
}

impl Config {
    /// Suggested client download concurrency, clamped to a sane range.
    pub fn download_concurrency(&self) -> u8 {
        self.server.download_concurrency.clamp(1, 16)
    }

    /// Public URL of the yggdrasil endpoint, as seen by clients and
    /// authlib-injector. Falls back to the bind address while the placeholder
    /// default is still in place.
    pub fn yggdrasil_base_url(&self) -> String {
        let configured = self.yggdrasil.base_url.trim().trim_end_matches('/');
        if configured.is_empty() || configured == crate::yggdrasil::DEFAULT_BASE_URL {
            // Prefer the scheme the daemon actually serves.
            let scheme = if self.network.tls.is_some() {
                "https"
            } else {
                "http"
            };
            format!(
                "{scheme}://{}:{}/yggdrasil",
                self.network.host.trim(),
                self.network.port
            )
        } else {
            configured.to_string()
        }
    }

    pub fn load(paths: &SyncraftPaths) -> Option<Self> {
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

        if let Some(modrinth_api) = &config.network.modrinth_api {
            mod_type::configure_modrinth_api(modrinth_api.trim_matches('/'));
        }
        Some(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn config_default_values() {
        let config = Config::default();
        assert_eq!(config.info.name, "<name>");
        assert_eq!(config.info.vanilla_version, "0.00.0");
        assert!(config.sync.is_empty());
    }

    #[test]
    fn yggdrasil_base_url_prefers_configured_value() {
        let mut config = Config::default();
        config.yggdrasil.base_url = "https://mc.example.com/yggdrasil/".into();
        assert_eq!(
            config.yggdrasil_base_url(),
            "https://mc.example.com/yggdrasil"
        );
    }

    #[test]
    fn yggdrasil_base_url_falls_back_to_bind_address() {
        let config = Config::default();
        assert_eq!(
            config.yggdrasil_base_url(),
            "http://127.0.0.1:3000/yggdrasil"
        );
    }

    #[test]
    fn parses_documented_config() {
        // Mirrors `syncraft-docs/syncraft/quick-start/config-file.md`; keeps the
        // documentation and the deserializer in sync.
        let config: Config = toml::from_str(
            r#"
forbid = ["mods/old-client-only-mod.jar"]

[info]
name = "MyServer"
id = "myserver"
vanilla_version = "1.21.1"

[info.loader.Fabric]
loader_version = "0.16.10"

[network]
host = "127.0.0.1"
port = 3000
sign_manifest = true

[network.tls]
cert = "certs/fullchain.pem"
key = "certs/privkey.pem"

[server]
snapshot_retention = 8
download_concurrency = 2

[[sync]]
source = "mods/"
target = "mods/"
mode = "minimal"
is_mod_dir = true

[yggdrasil]
base_url = "https://mc.example.com/yggdrasil"
skin_domains = ["example.com"]
identity_falsification = false
"#,
        )
        .unwrap();

        assert!(config.network.sign_manifest);
        assert_eq!(
            config.network.tls.as_ref().map(|tls| tls.cert.as_path()),
            Some(std::path::Path::new("certs/fullchain.pem"))
        );
        assert_eq!(config.server.snapshot_retention, 8);
        assert_eq!(config.download_concurrency(), 2);
        assert_eq!(config.forbid.len(), 1);
        assert_eq!(config.info.id, "myserver");
        assert_eq!(
            config.yggdrasil_base_url(),
            "https://mc.example.com/yggdrasil"
        );
    }

    #[test]
    fn download_concurrency_is_clamped() {
        let mut config = Config::default();
        config.server.download_concurrency = 99;
        assert_eq!(config.download_concurrency(), 16);
        config.server.download_concurrency = 0;
        assert_eq!(config.download_concurrency(), 1);
    }

    #[test]
    fn sync_mode_names() {
        let parse = |mode: &str| -> Result<SyncObject, toml::de::Error> {
            toml::from_str(&format!(
                "source = \"mods\"\ntarget = \"mods\"\nmode = \"{mode}\"\n"
            ))
        };

        assert_eq!(parse("ifmissing").unwrap().mode, SyncMode::IfMissing);
        assert_eq!(
            toml::Value::try_from(SyncMode::IfMissing).unwrap().as_str(),
            Some("ifmissing")
        );
        assert_eq!(parse("minimal").unwrap().mode, SyncMode::Minimal);
        assert_eq!(parse("strict").unwrap().mode, SyncMode::Strict);
        assert_eq!(parse("forbidden").unwrap().mode, SyncMode::Forbidden);

        // `default` is not accepted; use `ifmissing`.
        assert!(parse("default").is_err());

        // Omitting `mode` keeps the documented default.
        let object: SyncObject = toml::from_str("source = \"mods\"\ntarget = \"mods\"").unwrap();
        assert_eq!(object.mode, SyncMode::Strict);
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
id = "testserver"
vanilla_version = "1.20.1"
loader = "Vanilla"

[network]
host = "127.0.0.1"
port = 3000

[[sync]]
source = "mods"
target = "mods"

[yggdrasil]
base_url = "http://localhost/yggdrasil"
skin_domains = ["example.com"]
identity_falsification = false
"#;
        fs::write(paths.config_path(), toml_content).unwrap();

        let config = Config::load(&paths).unwrap();
        assert_eq!(config.info.name, "TestServer");
        assert_eq!(config.info.id, "testserver");
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

    #[tokio::test]
    async fn check_ser() {
        let config = Config {
            sync: vec![SyncObject {
                source: "test".into(),
                target: "test".into(),
                mode: SyncMode::IfMissing,
                is_mod_dir: false,
                target_is_dir: None,
            }],
            ..Default::default()
        };
        let config_str = toml::to_string_pretty(&config).unwrap();
        println!("{}", config_str)
    }
}
