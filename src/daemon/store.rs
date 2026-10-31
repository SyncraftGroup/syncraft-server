//! Persistent manifest snapshots.
//!
//! Every distinct file set gets its own snapshot keyed by the manifest content
//! hash, so `version_hash` always maps back to an exact file list. This is what
//! makes `?version=<hash>` (rollback / pinning) and reproducible updates
//! possible; without it, `version_hash` was just the last git commit and could
//! not describe the files that were actually served.

use std::path::PathBuf;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use syncraft_protocol::Manifest;

use crate::{error::Error, paths::SyncraftPaths};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotInfo {
    pub version_hash: String,
    #[serde(default)]
    pub commit: String,
    #[serde(default)]
    pub created_at: String,
    pub files: usize,
}

pub struct ManifestStore {
    dir: PathBuf,
}

impl ManifestStore {
    pub fn new(paths: &SyncraftPaths) -> Self {
        Self {
            dir: paths.manifests_dir(),
        }
    }

    fn index_path(&self) -> PathBuf {
        self.dir.join("index.json")
    }

    /// Reject anything that is not a plain content hash before touching paths.
    fn snapshot_path(&self, hash: &str) -> Option<PathBuf> {
        if hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()) {
            Some(self.dir.join(format!("{hash}.json")))
        } else {
            None
        }
    }

    /// Persist a snapshot (idempotent) and update the index.
    ///
    /// `retention` limits how many snapshots are kept; `0` keeps all of them.
    pub fn persist(&self, manifest: &Manifest, retention: usize) -> Result<(), Error> {
        let Some(path) = self.snapshot_path(&manifest.version_hash) else {
            return Err(Error::Server(format!(
                "invalid manifest hash: {}",
                manifest.version_hash
            )));
        };
        std::fs::create_dir_all(&self.dir)?;

        if !path.exists() {
            std::fs::write(&path, serde_json::to_string(manifest)?)?;
        }

        let mut index = self.read_index();
        if !index
            .iter()
            .any(|snapshot| snapshot.version_hash == manifest.version_hash)
        {
            index.insert(
                0,
                SnapshotInfo {
                    version_hash: manifest.version_hash.clone(),
                    commit: manifest.commit.clone(),
                    created_at: Utc::now().to_rfc3339(),
                    files: manifest.files.len(),
                },
            );
        }

        while retention > 0 && index.len() > retention {
            if let Some(stale) = index.pop()
                && let Some(stale_path) = self.snapshot_path(&stale.version_hash)
            {
                let _ = std::fs::remove_file(stale_path);
            }
        }

        std::fs::write(self.index_path(), serde_json::to_string_pretty(&index)?)?;
        Ok(())
    }

    pub fn load(&self, hash: &str) -> Result<Option<Manifest>, Error> {
        let Some(path) = self.snapshot_path(hash) else {
            return Ok(None);
        };
        match std::fs::read_to_string(&path) {
            Ok(content) => Ok(Some(serde_json::from_str(&content)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Snapshots, newest first.
    pub fn list(&self) -> Vec<SnapshotInfo> {
        self.read_index()
    }

    fn read_index(&self) -> Vec<SnapshotInfo> {
        std::fs::read_to_string(self.index_path())
            .ok()
            .and_then(|content| serde_json::from_str(&content).ok())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use syncraft_protocol::{Loader, MANIFEST_SCHEMA, VersionInfo};

    use super::*;

    fn temp_paths(name: &str) -> SyncraftPaths {
        let root = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&root);
        let paths = SyncraftPaths::new(root);
        paths.create_dirs().unwrap();
        paths
    }

    fn manifest_with(hash: &str) -> Manifest {
        Manifest {
            schema: MANIFEST_SCHEMA,
            version_hash: hash.to_string(),
            commit: "deadbeef".into(),
            version: VersionInfo {
                name: "test".into(),
                id: "test".into(),
                vanilla_version: "1.21.1".into(),
                loader: Loader::Vanilla,
            },
            download_concurrency: None,
            signature: None,
            files: BTreeMap::new(),
        }
    }

    #[test]
    fn retention_prunes_old_snapshots() {
        let paths = temp_paths("syncraft_test_manifest_store_retention");
        let store = ManifestStore::new(&paths);

        for n in 0..5u8 {
            let hash = format!("{n:02x}").repeat(32);
            store.persist(&manifest_with(&hash), 2).unwrap();
        }

        let snapshots = store.list();
        assert_eq!(snapshots.len(), 2);
        // The two newest are kept and still loadable.
        for snapshot in &snapshots {
            assert!(store.load(&snapshot.version_hash).unwrap().is_some());
        }

        let _ = std::fs::remove_dir_all(paths.root());
    }

    #[test]
    fn snapshots_round_trip() {
        let paths = temp_paths("syncraft_test_manifest_store");
        let store = ManifestStore::new(&paths);
        let hash = "a".repeat(64);

        store.persist(&manifest_with(&hash), 32).unwrap();
        assert_eq!(store.list().len(), 1);
        let loaded = store.load(&hash).unwrap().unwrap();
        assert_eq!(loaded.version_hash, hash);
        assert_eq!(loaded.commit, "deadbeef");

        // Unknown / malformed hashes resolve to nothing instead of escaping.
        assert!(store.load(&"b".repeat(64)).unwrap().is_none());
        assert!(store.load("../../etc/passwd").unwrap().is_none());

        let _ = std::fs::remove_dir_all(paths.root());
    }
}
