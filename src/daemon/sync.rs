use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use crate::{config::Config, error::Error, log_debug, log_info};
use blake3::Hash;
use futures::stream::{FuturesUnordered, StreamExt};
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

pub async fn file_mapping(config: &Config) -> Result<HashMap<PathBuf, SyncFile>, Error> {
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
            filelist.extend(expand_dir(source, target, object.mode)?);
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
