//! Build the optional client theme package advertised in the manifest.
//!
//! A theme is a directory containing `theme.json` plus static assets, or a zip
//! of such a directory. The server stores a deterministic zip under
//! `syncraft/data/themes/<blake3>.zip` and serves it at `/theme/<hash>`.

use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

use syncraft_protocol::{ThemeDescriptor, ThemeInfo};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::config::ThemeConfig;
use crate::error::Error;
use crate::paths::SyncraftPaths;

/// Descriptor file every theme package must contain.
pub const DESCRIPTOR: &str = "theme.json";

/// Package the configured theme (if any) and return the manifest entry.
pub fn build(
    paths: &SyncraftPaths,
    config: Option<&ThemeConfig>,
) -> Result<Option<ThemeInfo>, Error> {
    let Some(config) = config else {
        return Ok(None);
    };

    let source = paths.resolve(&config.path);
    if !source.exists() {
        return Err(Error::Server(format!(
            "theme path not found: {}",
            source.display()
        )));
    }

    let (bytes, name) = if source.is_dir() {
        let name = name_from_dir(&source)?;
        (zip_dir(&source)?, name)
    } else {
        let bytes = fs::read(&source)?;
        let name = name_from_zip(&bytes).unwrap_or_else(|_| file_stem(&source));
        (bytes, name)
    };

    let hash = blake3::hash(&bytes).to_hex().to_string();
    let dir = paths.data().join("themes");
    fs::create_dir_all(&dir)?;
    let target = dir.join(format!("{hash}.zip"));
    if !target.exists() {
        fs::write(&target, &bytes)?;
    }

    Ok(Some(ThemeInfo { hash, name }))
}

/// Path of a packaged theme, used by the HTTP route.
pub fn package_path(paths: &SyncraftPaths, hash: &str) -> Option<PathBuf> {
    if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let path = paths.data().join("themes").join(format!("{hash}.zip"));
    path.exists().then_some(path)
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("Theme")
        .to_string()
}

fn name_from_dir(dir: &Path) -> Result<String, Error> {
    let text = fs::read_to_string(dir.join(DESCRIPTOR)).map_err(|e| {
        Error::Server(format!(
            "theme descriptor {} is missing or unreadable: {e}",
            dir.join(DESCRIPTOR).display()
        ))
    })?;
    let descriptor: ThemeDescriptor = serde_json::from_str(&text)?;
    Ok(descriptor
        .name
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| file_stem(dir)))
}

fn name_from_zip(bytes: &[u8]) -> Result<String, Error> {
    let mut archive = ZipArchive::new(Cursor::new(bytes))?;
    let mut file = archive
        .by_name(DESCRIPTOR)
        .map_err(|_| Error::Server("theme zip is missing theme.json".into()))?;
    let mut text = String::new();
    file.read_to_string(&mut text)?;
    let descriptor: ThemeDescriptor = serde_json::from_str(&text)?;
    Ok(descriptor
        .name
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "Theme".into()))
}

fn zip_dir(root: &Path) -> Result<Vec<u8>, Error> {
    let mut entries = Vec::new();
    collect(root, root, &mut entries)?;
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    // Fixed timestamp keeps the archive (and therefore its hash) stable when
    // the theme content did not change.
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .last_modified_time(
            zip::DateTime::from_date_and_time(1980, 1, 1, 0, 0, 0)
                .map_err(|e| Error::Server(format!("invalid zip timestamp: {e}")))?,
        );

    for (relative, absolute) in entries {
        writer.start_file(relative, options)?;
        let data = fs::read(absolute)?;
        writer.write_all(&data)?;
    }

    Ok(writer.finish()?.into_inner())
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) -> Result<(), Error> {
    let mut children: Vec<_> = fs::read_dir(dir)?.collect::<Result<Vec<_>, _>>()?;
    children.sort_by_key(|entry| entry.file_name());

    for entry in children {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            collect(root, &path, out)?;
        } else if metadata.is_file() {
            let relative = path.strip_prefix(root).unwrap_or(&path);
            let name = relative
                .components()
                .map(|component| component.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            out.push((name, path));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ThemeConfig;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "syncraft-theme-build-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn directory_theme_has_a_stable_hash() {
        let root = temp("dir");
        let theme = root.join("theme");
        fs::create_dir_all(theme.join("fonts")).unwrap();
        fs::write(
            theme.join(DESCRIPTOR),
            r##"{"name":"Aurora","colors":{"paper":"#fafafa"}}"##,
        )
        .unwrap();
        fs::write(theme.join("fonts").join("ui.woff2"), b"font").unwrap();

        let paths = SyncraftPaths::new(root.clone());
        let config = ThemeConfig {
            path: PathBuf::from("theme"),
        };

        let first = build(&paths, Some(&config)).unwrap().unwrap();
        let second = build(&paths, Some(&config)).unwrap().unwrap();
        assert_eq!(first.hash, second.hash);
        assert_eq!(first.name, "Aurora");
        assert!(package_path(&paths, &first.hash).is_some());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn zip_theme_is_used_as_is() {
        let root = temp("zip");
        let theme = root.join("theme");
        fs::create_dir_all(&theme).unwrap();
        fs::write(theme.join(DESCRIPTOR), r#"{"name":"Zipped"}"#).unwrap();
        let bytes = zip_dir(&theme).unwrap();
        let archive = root.join("theme.zip");
        fs::write(&archive, &bytes).unwrap();

        let paths = SyncraftPaths::new(root.clone());
        let config = ThemeConfig { path: archive };
        let info = build(&paths, Some(&config)).unwrap().unwrap();
        assert_eq!(info.name, "Zipped");
        assert_eq!(info.hash, blake3::hash(&bytes).to_hex().to_string());
        assert!(package_path(&paths, &info.hash).is_some());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn missing_descriptor_is_an_error() {
        let root = temp("missing");
        let theme = root.join("theme");
        fs::create_dir_all(&theme).unwrap();
        fs::write(theme.join("style.css"), b"body{}").unwrap();

        let paths = SyncraftPaths::new(root.clone());
        let config = ThemeConfig {
            path: PathBuf::from("theme"),
        };
        assert!(build(&paths, Some(&config)).is_err());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn invalid_hash_cannot_escape_the_theme_directory() {
        let root = temp("hash");
        let paths = SyncraftPaths::new(root.clone());
        assert!(package_path(&paths, "../theme").is_none());
        assert!(package_path(&paths, &"a".repeat(64)).is_none());
        let _ = fs::remove_dir_all(root);
    }
}
