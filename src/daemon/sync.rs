use std::{
    collections::HashMap,
    io::Write,
    path::{Path, PathBuf},
};

use crate::{config::Config, error::Error, log_debug, log_info, paths::SyncraftPaths};
use blake3::Hash;
use futures::stream::{FuturesUnordered, StreamExt};
use serde::{Deserialize, Serialize};
use syncraft_protocol::{FileState, SyncFile, SyncMode};

pub struct FileMeta {
    source: PathBuf,
    target: PathBuf,
    mode: SyncMode,
}

pub fn expand_dir(source: &Path, target: &Path, mode: SyncMode) -> Result<Vec<FileMeta>, Error> {
    let mut files = Vec::new();

    for entry in source.read_dir()? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() {
            let relative = path
                .strip_prefix(source)
                .map_err(|e| Error::Server(e.to_string()))?
                .to_path_buf();
            files.push(FileMeta {
                source: path,
                target: target.join(relative),
                mode,
            });
        } else if path.is_dir() {
            let relative = path
                .strip_prefix(source)
                .map_err(|e| Error::Server(e.to_string()))?
                .to_path_buf();
            files.extend(expand_dir(&path, &target.join(relative), mode)?);
        }
    }
    Ok(files)
}

pub async fn file_mapping(
    config: &Config,
    paths: &SyncraftPaths,
) -> Result<HashMap<PathBuf, SyncFile>, Error> {
    let mut filelist = Vec::new();

    for object in &config.sync {
        let source = &object.source;
        let target = &object.target;
        if source.is_file() {
            let target_path = if target.is_dir() || target.extension().is_none() {
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
            });
        } else if source.is_dir() {
            let mut files = expand_dir(source, target, object.mode)?;
            if object.is_mod_dir {
                files = resolve_mods(files, paths)?;
            }
            filelist.extend(files);
        }
    }

    log_info!("Generating hashes...");

    let mut futures = FuturesUnordered::new();

    for meta in filelist {
        futures.push(async move {
            let state = FileState::from_path(&meta.source).await?;
            Ok::<(PathBuf, SyncFile), Error>((
                meta.target,
                SyncFile {
                    state: Some(state),
                    mode: meta.mode,
                },
            ))
        });
    }

    let mut files = HashMap::new();
    while let Some(result) = futures.next().await {
        let (path, file) = result?;
        log_debug!("Hash generated for {}", path.to_string_lossy());
        files.insert(path, file);
    }
    Ok(files)
}

pub fn hash_mapping(file_mapping: &HashMap<PathBuf, SyncFile>) -> HashMap<Hash, PathBuf> {
    let mut hash_mapping = HashMap::new();

    for (path, file) in file_mapping {
        if let Some(state) = &file.state {
            hash_mapping.insert(state.hash.clone(), path.clone());
        }
    }
    hash_mapping
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ModFileType {
    Sync,
    Ignore,
    Unknow,
}

pub fn resolve_mods(files: Vec<FileMeta>, paths: &SyncraftPaths) -> Result<Vec<FileMeta>, Error> {
    let cache_str = std::fs::read_to_string(paths.mod_type_cache()).unwrap_or_default();
    let mut cache: HashMap<PathBuf, ModFileType> =
        serde_json::from_str(&cache_str).unwrap_or_default();

    let mut result = Vec::new();

    for file in files {
        if let Some(mod_type) = cache.get(&file.source)
            && mod_type != &ModFileType::Ignore
        {
            result.push(file);
        } else {
            let _ = cache.insert(file.source.clone(), ModFileType::Unknow);
            result.push(file);
        }
    }

    let cache_str = serde_json::to_string_pretty(&cache)?;
    let mut f = std::fs::File::create(paths.mod_type_cache())?;
    write!(f, "{}", cache_str)?;

    Ok(result)
}
