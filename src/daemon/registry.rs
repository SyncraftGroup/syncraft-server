//! Registry of Minecraft server roots managed by one daemon process.
//!
//! Stored in the per-user data directory (`servers.toml`), independent of any
//! individual server. Each entry points at a directory that contains
//! `syncraft.toml` and its own `syncraft/` workspace.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::error::Error;
use crate::paths::{GlobalPaths, SyncraftPaths};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerEntry {
    /// Stable tenant name. Usually `info.id` from the server's config.
    pub name: String,
    /// Absolute path to the Minecraft server directory.
    pub path: PathBuf,
}

/// User-facing failures when managing the registry.
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    TomlDe(#[from] toml::de::Error),
    #[error("{0}")]
    TomlSer(#[from] toml::ser::Error),
    #[error("not initialized: {0}")]
    NotInitialized(PathBuf),
    #[error("server name must not be empty")]
    EmptyName,
    #[error("{path} is already registered as '{name}'")]
    DuplicatePath { path: PathBuf, name: String },
    #[error("server name '{name}' is already used by {path}")]
    DuplicateName { name: String, path: PathBuf },
    #[error("invalid path {path}: {reason}")]
    InvalidPath { path: PathBuf, reason: String },
}

impl From<RegistryError> for Error {
    fn from(value: RegistryError) -> Self {
        Error::Daemon(value.to_string())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Registry {
    #[serde(default)]
    pub servers: Vec<ServerEntry>,
}

impl Registry {
    pub fn load(global: &GlobalPaths) -> Result<Self, RegistryError> {
        let path = global.registry_path();
        match std::fs::read_to_string(&path) {
            Ok(content) => Ok(toml::from_str(&content)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save(&self, global: &GlobalPaths) -> Result<(), RegistryError> {
        std::fs::create_dir_all(global.root())?;
        let content = toml::to_string_pretty(self)?;
        std::fs::write(global.registry_path(), content)?;
        Ok(())
    }

    /// Register `path`. `name` defaults to the server's `info.id`.
    pub fn add(&mut self, path: &Path, name: Option<String>) -> Result<ServerEntry, RegistryError> {
        let root = canonical_dir(path)?;
        let paths = SyncraftPaths::new(root.clone());
        let config =
            Config::load(&paths).ok_or_else(|| RegistryError::NotInitialized(root.clone()))?;

        let name = name
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| config.info.id.trim().to_string());
        if name.is_empty() {
            return Err(RegistryError::EmptyName);
        }

        if let Some(existing) = self.servers.iter().find(|entry| entry.path == root) {
            return Err(RegistryError::DuplicatePath {
                path: root,
                name: existing.name.clone(),
            });
        }
        if let Some(existing) = self.servers.iter().find(|entry| entry.name == name) {
            return Err(RegistryError::DuplicateName {
                name,
                path: existing.path.clone(),
            });
        }

        let entry = ServerEntry { name, path: root };
        self.servers.push(entry.clone());
        Ok(entry)
    }

    /// Remove by name or path.
    pub fn remove(&mut self, key: &str) -> Option<ServerEntry> {
        let index = self.servers.iter().position(|entry| entry.matches(key))?;
        Some(self.servers.remove(index))
    }

    /// Find by name or path.
    #[allow(dead_code)]
    pub fn find(&self, key: &str) -> Option<&ServerEntry> {
        self.servers.iter().find(|entry| entry.matches(key))
    }
}

impl ServerEntry {
    pub(crate) fn matches(&self, key: &str) -> bool {
        if self.name == key || self.path.as_os_str() == key {
            return true;
        }
        canonical_dir(Path::new(key))
            .map(|candidate| candidate == self.path)
            .unwrap_or(false)
    }
}

fn canonical_dir(path: &Path) -> Result<PathBuf, RegistryError> {
    let canonical = path
        .canonicalize()
        .map_err(|e| RegistryError::InvalidPath {
            path: path.to_path_buf(),
            reason: e.to_string(),
        })?;
    if !canonical.is_dir() {
        return Err(RegistryError::InvalidPath {
            path: canonical,
            reason: "not a directory".into(),
        });
    }
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_rejects_duplicate_path_and_name() {
        let root = std::env::temp_dir().join("syncraft_test_registry");
        let _ = std::fs::remove_dir_all(&root);
        let server = root.join("mc");
        std::fs::create_dir_all(&server).unwrap();
        let paths = SyncraftPaths::new(server.clone());
        paths.create_dirs().unwrap();
        std::fs::write(
            paths.config_path(),
            "[info]\nname = \"Test\"\nid = \"test\"\nvanilla_version = \"1.21.1\"\nloader = \"Vanilla\"\n[network]\nhost = \"127.0.0.1\"\nport = 3000\n[[sync]]\nsource = \".\"\ntarget = \".\"\n[yggdrasil]\nbase_url = \"http://localhost/yggdrasil\"\nskin_domains = []\nidentity_falsification = false\n",
        )
        .unwrap();

        let mut registry = Registry::default();
        let entry = registry.add(&server, None).unwrap();
        assert_eq!(entry.name, "test");
        assert!(registry.add(&server, None).is_err());
        assert!(registry.add(&server, Some("test".into())).is_err());
        assert!(registry.remove("test").is_some());

        let _ = std::fs::remove_dir_all(&root);
    }
}
