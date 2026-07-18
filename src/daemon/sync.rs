use std::{collections::HashMap, path::{Path, PathBuf}};

use blake3::Hash;
use futures::stream::{FuturesUnordered, StreamExt};
use syncraft_protocol::{FileState, SyncFile, SyncMode};
use crate::{config::Config, error::Error, log_debug, log_info};

pub struct FileMeta {
    path: PathBuf,
    mode: SyncMode
}

pub fn expand_dir(path: &Path, mode: SyncMode) -> Result<Vec<FileMeta>, Error> {

    let mut files = Vec::new();

    for entry in path.read_dir()? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() {
            files.push(FileMeta { path, mode: mode });
        } else if path.is_dir() {
            files.extend(expand_dir(&path, mode)?);
        }
    }
    Ok(files)
}

pub async fn file_mapping(config: &Config) -> Result<HashMap<PathBuf, SyncFile>, Error> {
    
    let mut filelist = Vec::new();

    for object in &config.sync {
        let path = &object.path;
        if path.is_file() {
            filelist.push(FileMeta { path: path.clone(), mode: object.mode });
        } else if path.is_dir() {
            filelist.extend(expand_dir(path, object.mode)?);
        }
    }

    log_info!("Generating hashes...");

    let mut futures = FuturesUnordered::new();

    for meta in filelist {
        futures.push(async move {
            let state = FileState::from_path(&meta.path).await?;
            Ok::<(PathBuf, SyncFile), Error>((
                meta.path,
                SyncFile {
                state: Some(state),
                mode: meta.mode
            }))
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
            hash_mapping.insert(
                state.hash.clone(),
                path.clone()
            );
        }
    }
    hash_mapping
}