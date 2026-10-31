use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io::Write,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use crate::{config::Config, error::Error, log_debug, log_info, log_warn, paths::SyncraftPaths};
use blake3::Hash;
use futures::stream::{FuturesUnordered, StreamExt};
use serde::{Deserialize, Serialize};
use syncraft_protocol::{FileState, SyncFile, SyncMode};

/// Manifest entries, keyed by the path the file has on the **client**.
/// A sorted map keeps the manifest (and therefore its content hash) stable.
pub type FileMap = BTreeMap<PathBuf, SyncFile>;

/// Download lookup: content hash → every file on this server that provides it.
///
/// There can be several (identical content at different paths); the download
/// handler tries them in order so one deleted copy does not break the others.
pub type HashSourceMap = HashMap<Hash, Vec<PathBuf>>;

pub struct FileMeta {
    source: PathBuf,
    target: PathBuf,
    mode: SyncMode,
    is_mod_dir: bool,
}

pub fn expand_dir(
    source: &Path,
    target: &Path,
    mode: SyncMode,
    is_mod_dir: bool,
) -> Result<Vec<FileMeta>, Error> {
    let mut files = Vec::new();

    for entry in source.read_dir()? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        // Do not follow symlinks: they can point outside the workspace and
        // create cycles.
        if file_type.is_symlink() {
            log_debug!("Skipping symlink {}", path.display());
            continue;
        }
        if file_type.is_file() {
            let relative = path
                .strip_prefix(source)
                .map_err(|e| Error::Server(e.to_string()))?
                .to_path_buf();
            files.push(FileMeta {
                source: path,
                target: target.join(relative),
                mode,
                is_mod_dir,
            });
        } else if file_type.is_dir() {
            let relative = path
                .strip_prefix(source)
                .map_err(|e| Error::Server(e.to_string()))?
                .to_path_buf();
            files.extend(expand_dir(&path, &target.join(relative), mode, is_mod_dir)?);
        }
    }
    Ok(files)
}

/// Whether `path` is a plain relative client path (`a/b/c`, no `..`, no root).
pub fn is_safe_client_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
}

fn modified_ns(metadata: &std::fs::Metadata) -> u128 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

/// Hash cache: avoids re-reading unchanged files on every daemon start.
///
/// Keyed by absolute path and invalidated by size or modification time.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct HashCache {
    #[serde(default)]
    entries: HashMap<PathBuf, HashCacheEntry>,
    #[serde(skip)]
    dirty: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct HashCacheEntry {
    size: u64,
    modified_ns: u128,
    /// Hex-encoded blake3 hash.
    hash: String,
}

impl HashCache {
    pub fn load(paths: &SyncraftPaths) -> Self {
        let content = std::fs::read_to_string(paths.hash_cache()).unwrap_or_default();
        serde_json::from_str(&content).unwrap_or_default()
    }

    pub fn save(&mut self, paths: &SyncraftPaths) -> Result<(), Error> {
        if !self.dirty {
            return Ok(());
        }
        if let Some(parent) = paths.hash_cache().parent()
            && !parent.exists()
        {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(paths.hash_cache(), serde_json::to_string(&self)?)?;
        self.dirty = false;
        Ok(())
    }

    fn lookup(&self, path: &Path, size: u64, modified: u128) -> Option<FileState> {
        let entry = self.entries.get(path)?;
        if entry.size != size || entry.modified_ns != modified {
            return None;
        }
        Some(FileState {
            size,
            hash: entry.hash.parse::<blake3::Hash>().ok()?,
        })
    }

    fn remember(&mut self, path: &Path, size: u64, modified: u128, hash: Hash) {
        self.entries.insert(
            path.to_path_buf(),
            HashCacheEntry {
                size,
                modified_ns: modified,
                hash: hash.to_hex().to_string(),
            },
        );
        self.dirty = true;
    }

    /// Drop entries for files that are no longer part of the sync set.
    fn retain_paths(&mut self, keep: &HashSet<PathBuf>) {
        let before = self.entries.len();
        self.entries.retain(|path, _| keep.contains(path));
        if self.entries.len() != before {
            self.dirty = true;
        }
    }
}

/// Mod filter result, keyed by the **content hash** of the jar.
///
/// Keying by path was wrong: replacing a jar in place (new download, same
/// file name) kept the old server-only/client-only decision forever.
pub type ModCache = HashMap<String, ModFileType>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ModFileType {
    Sync,
    Ignore,
    Unknown,
}

pub fn load_mod_cache(paths: &SyncraftPaths) -> ModCache {
    let content = std::fs::read_to_string(paths.mod_type_cache()).unwrap_or_default();
    serde_json::from_str(&content).unwrap_or_default()
}

pub fn save_mod_cache(paths: &SyncraftPaths, cache: &ModCache) -> Result<(), Error> {
    let content = serde_json::to_string_pretty(cache)?;
    let mut file = std::fs::File::create(paths.mod_type_cache())?;
    write!(file, "{}", content)?;
    Ok(())
}

struct Prepared {
    meta: FileMeta,
    size: u64,
    modified_ns: u128,
    state: Option<FileState>,
}

/// Build the manifest and the download lookup for the given configuration.
///
/// `Config::sync` paths are relative to the server root, so they are resolved
/// explicitly instead of relying on the process working directory.
pub async fn file_mapping(
    config: &Config,
    paths: &SyncraftPaths,
    hash_cache: &mut HashCache,
) -> Result<(FileMap, HashSourceMap), Error> {
    let mut filelist = Vec::new();

    for object in &config.sync {
        let source = paths.resolve(&object.source);
        let target = &object.target;
        if source.is_file() {
            let target_is_dir = object
                .target_is_dir
                .unwrap_or_else(|| target.is_dir() || target.extension().is_none());
            let target_path = if target_is_dir {
                target.join(source.file_name().ok_or_else(|| {
                    Error::Server(format!("Source has no file name: {}", source.display()))
                })?)
            } else {
                target.clone()
            };
            filelist.push(FileMeta {
                source: source.clone(),
                target: target_path,
                mode: object.mode,
                is_mod_dir: object.is_mod_dir,
            });
        } else if source.is_dir() {
            filelist.extend(expand_dir(&source, target, object.mode, object.is_mod_dir)?);
        } else {
            log_info!("Sync source not found: {}", source.display());
        }
    }

    // 1. Stat everything and reuse cached hashes when size and mtime match.
    let mut prepared = Vec::with_capacity(filelist.len());
    for meta in filelist {
        let metadata = match tokio::fs::metadata(&meta.source).await {
            Ok(metadata) => metadata,
            Err(e) => {
                log_warn!("Skipping {}: {e}", meta.source.display());
                continue;
            }
        };
        let size = metadata.len();
        let modified = modified_ns(&metadata);
        let state = hash_cache.lookup(&meta.source, size, modified);
        prepared.push(Prepared {
            meta,
            size,
            modified_ns: modified,
            state,
        });
    }

    // 2. Hash the cache misses concurrently.
    let mut futures = FuturesUnordered::new();
    for (index, item) in prepared.iter().enumerate() {
        if item.state.is_none() {
            let path = item.meta.source.clone();
            futures.push(async move { (index, FileState::hash(&path).await) });
        }
    }
    while let Some((index, result)) = futures.next().await {
        let hash = result?;
        let item = &mut prepared[index];
        item.state = Some(FileState {
            size: item.size,
            hash,
        });
        hash_cache.remember(&item.meta.source, item.size, item.modified_ns, hash);
    }

    // 3. Apply the mod filter (by content hash) and build the maps.
    let mut mod_cache = load_mod_cache(paths);
    let mut mod_cache_dirty = false;
    let mut files = FileMap::new();
    let mut sources = HashSourceMap::new();
    let mut hashed_sources = HashSet::with_capacity(prepared.len());

    for item in prepared {
        let state = item.state.expect("all states are filled in step 2");
        // Keep the hash-cache entry even when the mod filter excludes the file:
        // the cache is about staleness, not about what is published.
        hashed_sources.insert(item.meta.source.clone());

        if item.meta.is_mod_dir {
            let key = state.hash.to_hex().to_string();
            match mod_cache.get(&key) {
                Some(ModFileType::Ignore) => {
                    log_debug!("Excluding server-only mod {}", item.meta.source.display());
                    continue;
                }
                Some(_) => {}
                None => {
                    mod_cache.insert(key, ModFileType::Unknown);
                    mod_cache_dirty = true;
                }
            }
        }

        sources
            .entry(state.hash)
            .or_default()
            .push(item.meta.source);
        files.insert(
            item.meta.target,
            SyncFile {
                state: Some(state),
                mode: item.meta.mode,
            },
        );
    }

    // 4. Explicit "must not exist on the client" entries. These do not need a
    //    matching file on the server.
    for path in &config.forbid {
        if !is_safe_client_path(path) {
            log_warn!("Ignoring unsafe forbid path: {}", path.display());
            continue;
        }
        files.insert(
            path.clone(),
            SyncFile {
                state: None,
                mode: SyncMode::Forbidden,
            },
        );
    }

    if mod_cache_dirty {
        save_mod_cache(paths, &mod_cache)?;
    }
    hash_cache.retain_paths(&hashed_sources);
    hash_cache.save(paths)?;

    Ok((files, sources))
}

/// All `.jar` files inside configured mod directories.
pub fn mod_jars(config: &Config, paths: &SyncraftPaths) -> Vec<PathBuf> {
    fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                collect(&path, out);
            } else if path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("jar"))
            {
                out.push(path);
            }
        }
    }

    let mut jars = Vec::new();
    for object in config.sync.iter().filter(|object| object.is_mod_dir) {
        let source = paths.resolve(&object.source);
        if source.is_dir() {
            collect(&source, &mut jars);
        } else if source.is_file()
            && source
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("jar"))
        {
            jars.push(source);
        }
    }
    jars.sort();
    jars
}

/// Blocking blake3 hash, used by the publish flow.
pub fn hash_file_blocking(path: &Path) -> Result<Hash, Error> {
    use std::io::Read;

    let mut file = std::fs::File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0u8; 128 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, SyncObject};

    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[tokio::test]
    async fn file_mapping_resolves_sources_against_root() {
        let root = temp_root("syncraft_test_file_mapping");
        std::fs::create_dir_all(root.join("server_files")).unwrap();
        std::fs::write(root.join("server_files/ops.json"), b"[]").unwrap();

        let paths = SyncraftPaths::new(root.clone());
        paths.create_dirs().unwrap();

        let config = Config {
            sync: vec![SyncObject {
                source: "server_files/ops.json".into(),
                target: "ops.json".into(),
                mode: SyncMode::Strict,
                is_mod_dir: false,
                target_is_dir: None,
            }],
            ..Default::default()
        };

        let mut cache = HashCache::default();
        let (files, sources) = file_mapping(&config, &paths, &mut cache).await.unwrap();

        // Manifest keys are client paths, downloads are served from sources.
        let entry = files
            .get(Path::new("ops.json"))
            .expect("manifest keyed by client path");
        let state = entry.state.as_ref().unwrap();
        assert_eq!(
            sources.get(&state.hash),
            Some(&vec![root.join("server_files/ops.json")])
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn mod_filter_is_keyed_by_content() {
        let root = temp_root("syncraft_test_mod_filter");
        let paths = SyncraftPaths::new(root.clone());
        paths.create_dirs().unwrap();
        std::fs::create_dir_all(root.join("mods")).unwrap();

        let client_only = root.join("mods/client-only.jar");
        let common = root.join("mods/common.jar");
        std::fs::write(&client_only, b"client-only").unwrap();
        std::fs::write(&common, b"common").unwrap();

        let ignored_hash = blake3::hash(b"client-only").to_hex().to_string();
        save_mod_cache(
            &paths,
            &HashMap::from([(ignored_hash.clone(), ModFileType::Ignore)]),
        )
        .unwrap();

        let config = Config {
            sync: vec![SyncObject {
                source: "mods".into(),
                target: "mods".into(),
                mode: SyncMode::Strict,
                is_mod_dir: true,
                target_is_dir: None,
            }],
            ..Default::default()
        };

        let mut cache = HashCache::default();
        let (files, _) = file_mapping(&config, &paths, &mut cache).await.unwrap();

        assert!(!files.contains_key(Path::new("mods/client-only.jar")));
        assert!(files.contains_key(Path::new("mods/common.jar")));

        // The Ignore decision must be preserved, not rewritten to Unknown.
        let cache = load_mod_cache(&paths);
        assert_eq!(cache.get(&ignored_hash), Some(&ModFileType::Ignore));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn forbid_entries_do_not_need_a_server_file() {
        let root = temp_root("syncraft_test_forbid");
        let paths = SyncraftPaths::new(root.clone());
        paths.create_dirs().unwrap();

        let config = Config {
            forbid: vec!["mods/never-here.jar".into()],
            ..Default::default()
        };

        let mut cache = HashCache::default();
        let (files, _) = file_mapping(&config, &paths, &mut cache).await.unwrap();

        let entry = files.get(Path::new("mods/never-here.jar")).unwrap();
        assert_eq!(entry.mode, SyncMode::Forbidden);
        assert!(entry.state.is_none());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn hash_cache_reuses_unchanged_entries() {
        let root = temp_root("syncraft_test_hash_cache");
        std::fs::create_dir_all(root.join("files")).unwrap();
        std::fs::write(root.join("files/a.txt"), b"hello").unwrap();

        let paths = SyncraftPaths::new(root.clone());
        paths.create_dirs().unwrap();

        let config = Config {
            sync: vec![SyncObject {
                source: "files/a.txt".into(),
                target: "a.txt".into(),
                mode: SyncMode::Strict,
                is_mod_dir: false,
                target_is_dir: None,
            }],
            ..Default::default()
        };

        let mut cache = HashCache::default();
        let (files, _) = file_mapping(&config, &paths, &mut cache).await.unwrap();
        let expected = files
            .get(Path::new("a.txt"))
            .unwrap()
            .state
            .as_ref()
            .unwrap()
            .hash;

        let source = root.join("files/a.txt");
        let metadata = std::fs::metadata(&source).unwrap();
        let modified = modified_ns(&metadata);

        // Same size and mtime: the cached hash is reused.
        assert_eq!(
            cache
                .lookup(&source, metadata.len(), modified)
                .map(|s| s.hash),
            Some(expected)
        );
        // Different mtime: the entry is considered stale.
        assert_eq!(cache.lookup(&source, metadata.len(), modified + 1), None);
        // The cache is persisted next to the workspace.
        assert!(paths.hash_cache().exists());

        let _ = std::fs::remove_dir_all(&root);
    }
}
