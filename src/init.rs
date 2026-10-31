use std::path::{Path, PathBuf};

use console_info::Ui;
use syncraft_protocol::{Loader, VersionInfo};

use crate::config::Config;
use crate::error::Error;
use crate::git;
use crate::log_debug;
use crate::log_warn;
use crate::message::{MessageKey, translate};
use crate::paths::SyncraftPaths;
use crate::yggdrasil;

pub fn run(paths: SyncraftPaths, force: bool) -> Result<(), Error> {
    log_debug!("Init started (force={force})");

    let ui = Ui::get();

    if !looks_like_mc_server_dir(&paths.root())? {
        log_warn!("Not a Minecraft server directory");
        ui.error(&translate(&MessageKey::InitOutsideMinecraftServerDirectory));
        return Ok(());
    }

    if paths.workspace().exists() {
        if force && ui.confirm(&translate(&MessageKey::CleanUpWorkSpaceConfirm))? {
            cleanup_old_workspace(&paths)?;
            ui.ok(&translate(&MessageKey::Done));
        } else {
            ui.error(&translate(&MessageKey::AlreadyInitialized));
            return Ok(());
        }
    }

    ui.step(&translate(&MessageKey::Initializing));

    let name = ui.input(&translate(&MessageKey::InputServerName))?;

    let (vanilla_version, loader) = {
        let (vv, loader) = guess_version(paths.root());
        let vanilla_version = if let Some(v) = vv {
            log_debug!("Version guessed: MC {v}, loader {loader:?}");
            v
        } else {
            ui.input(&translate(&MessageKey::InputVanillaVersion))?
        };
        (vanilla_version, loader)
    };

    let version_info = VersionInfo {
        id: slugify(&name),
        name,
        vanilla_version,
        loader,
    };

    log_debug!(
        "Init config: {} (MC {}, {:?})",
        version_info.name,
        version_info.vanilla_version,
        version_info.loader
    );

    ui.plain(&translate(&MessageKey::VersionInfoDisplay(&version_info)));

    if !ui.confirm(&translate(&MessageKey::VersionInfoConfirm))?
        && !ui.confirm(&translate(&MessageKey::ContinueToInitializeConfirm))?
    {
        ui.warn(&translate(&MessageKey::InitializationCancelled));
        return Ok(());
    }

    paths.create_dirs()?;
    log_debug!("Workspace directories created");

    let config = Config {
        info: version_info,
        ..Default::default()
    };
    write_config(&paths, &config)?;
    log_debug!("Config written to {}", paths.config_path().display());

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| Error::Daemon(format!("Failed to create runtime: {}", e)))?;
    // Build the router only to initialise the database and signing key here;
    // the daemon builds its own instance when it starts listening.
    drop(rt.block_on(yggdrasil::init(&paths, &config.yggdrasil))?);

    git::run(paths, Some(git::GitAction::Init))?;

    ui.ok(&translate(&MessageKey::InitializationFinished));
    log_debug!("Init completed successfully");
    Ok(())
}

fn write_config(paths: &SyncraftPaths, config: &Config) -> Result<(), Error> {
    let content = toml::to_string_pretty(config)?;
    std::fs::write(paths.config_path(), content)?;
    Ok(())
}

/// Stable, filesystem-safe identifier derived from the display name.
///
/// The client uses this for version file names, so renaming the server later
/// does not orphan version JSONs/jars.
fn slugify(name: &str) -> String {
    let slug: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "syncraft".to_string()
    } else {
        slug
    }
}

fn cleanup_old_workspace(paths: &SyncraftPaths) -> Result<(), Error> {
    let to_remove: [PathBuf; 4] = [
        paths.workspace(),
        paths.config_path(),
        paths.root().join(".gitignore"),
        paths.root().join(".git"),
    ];
    for path in &to_remove {
        if path.exists() {
            if path.is_dir() {
                std::fs::remove_dir_all(path)?;
            } else {
                std::fs::remove_file(path)?;
            }
        }
    }
    log_debug!("Old workspace cleaned up");
    Ok(())
}

fn looks_like_mc_server_dir(cwd: &Path) -> Result<bool, Error> {
    Ok(cwd.join("server.properties").exists()
        && cwd.join("eula.txt").exists()
        && cwd.join("libraries").exists())
}

/// Guess the Minecraft version and mod loader from the server directory.
///
/// Version directories are scanned newest-first, so a leftover older loader
/// does not shadow the one that is actually installed.
pub fn guess_version(path: PathBuf) -> (Option<String>, Loader) {
    let lib_path = path.join("libraries");

    let neoforge_dir = lib_path.join("net").join("neoforged").join("neoforge");
    for version_str in version_dirs(&neoforge_dir) {
        if let Some(parsed) = parse_neoforge(&version_str) {
            return parsed;
        }
    }

    let forge_dir = lib_path.join("net").join("minecraftforge").join("forge");
    for version_str in version_dirs(&forge_dir) {
        if let Some(pos) = version_str.find('-') {
            return (
                Some(version_str[..pos].to_string()),
                Loader::Forge {
                    loader_version: version_str[pos + 1..].to_string(),
                },
            );
        }
    }

    let fabric_path = lib_path.join("net").join("fabricmc");
    if fabric_path.exists() {
        let loader_version = version_dirs(&fabric_path.join("fabric-loader"))
            .into_iter()
            .next();
        let vanilla_version = version_dirs(&fabric_path.join("intermediary"))
            .into_iter()
            .next();

        if let (Some(lv), Some(vv)) = (loader_version, vanilla_version) {
            return (Some(vv), Loader::Fabric { loader_version: lv });
        }
    }

    (None, Loader::Vanilla)
}

/// Directory names, newest version first (numeric-aware).
fn version_dirs(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
        .collect();
    names.sort_by(|a, b| version_cmp(b, a));
    names
}

fn version_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let key = |value: &str| -> Vec<u64> {
        value
            .split(|c: char| !c.is_ascii_digit())
            .filter(|part| !part.is_empty())
            .filter_map(|part| part.parse().ok())
            .collect()
    };
    key(a).cmp(&key(b))
}

/// Warn when `syncraft.toml` disagrees with the loader actually installed.
///
/// The config drives what clients install; if it drifts from the server, the
/// launcher and the server will never match.
pub fn check_server_version(config: &Config, paths: &SyncraftPaths) {
    let (detected_mc, detected_loader) = guess_version(paths.root());

    if let Some(detected_mc) = detected_mc
        && detected_mc != config.info.vanilla_version
    {
        log_warn!(
            "Version mismatch: server runs Minecraft {detected_mc}, syncraft.toml says {}",
            config.info.vanilla_version
        );
    }

    if !matches!(detected_loader, Loader::Vanilla)
        && detected_loader.tag() != config.info.loader.tag()
    {
        log_warn!(
            "Version mismatch: server runs {}, syncraft.toml says {}",
            detected_loader.tag(),
            config.info.loader.tag()
        );
    }
}

/// Parse a NeoForge version directory name.
///
/// Versions < 26 are prefixed with `1.` (e.g. `20.1.100` → `1.20.1`).
/// Versions >= 26 keep their major number.
fn parse_neoforge(version_str: &str) -> Option<(Option<String>, Loader)> {
    let pos_2 = version_str.rfind('.')?;
    let pos_1 = version_str[..pos_2].find('.')?;
    let main_num: usize = version_str[..pos_1].parse().ok()?;

    let vanilla_version = if main_num < 26 {
        format!("1.{}", &version_str[..pos_2])
    } else {
        version_str[..pos_2].to_string()
    };

    Some((
        Some(vanilla_version),
        Loader::NeoForge {
            loader_version: version_str.to_string(),
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::guess_version;
    use std::fs;
    use std::path::PathBuf;
    use syncraft_protocol::Loader;

    fn make_lib(path: &PathBuf, mod_path: &str, version: &str) {
        let dir = path.join("libraries").join(mod_path).join(version);
        fs::create_dir_all(&dir).unwrap();
    }

    #[test]
    fn guess_neoforge_version() {
        let tmp = std::env::temp_dir().join("syncraft_test_neoforge");
        let _ = fs::remove_dir_all(&tmp);
        make_lib(&tmp, "net/neoforged/neoforge", "20.1.100");
        let (ver, loader) = guess_version(tmp.clone());
        assert_eq!(ver, Some("1.20.1".into()));
        assert!(matches!(loader, Loader::NeoForge { .. }));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn guess_neoforge_post_26() {
        let tmp = std::env::temp_dir().join("syncraft_test_neoforge26");
        let _ = fs::remove_dir_all(&tmp);
        make_lib(&tmp, "net/neoforged/neoforge", "26.1.50");
        let (ver, loader) = guess_version(tmp.clone());
        assert_eq!(ver, Some("26.1".into()));
        assert!(matches!(loader, Loader::NeoForge { .. }));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn guess_forge_version() {
        let tmp = std::env::temp_dir().join("syncraft_test_forge");
        let _ = fs::remove_dir_all(&tmp);
        make_lib(&tmp, "net/minecraftforge/forge", "1.20.1-47.2.0");
        let (ver, loader) = guess_version(tmp.clone());
        assert_eq!(ver, Some("1.20.1".into()));
        assert!(matches!(loader, Loader::Forge { .. }));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn guess_fabric_version() {
        let tmp = std::env::temp_dir().join("syncraft_test_fabric");
        let _ = fs::remove_dir_all(&tmp);
        make_lib(&tmp, "net/fabricmc/fabric-loader", "0.15.11");
        make_lib(&tmp, "net/fabricmc/intermediary", "1.20.1");
        let (ver, loader) = guess_version(tmp.clone());
        assert_eq!(ver, Some("1.20.1".into()));
        assert!(matches!(loader, Loader::Fabric { .. }));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn guess_vanilla() {
        let tmp = std::env::temp_dir().join("syncraft_test_vanilla");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let (ver, loader) = guess_version(tmp.clone());
        assert_eq!(ver, None);
        assert!(matches!(loader, Loader::Vanilla));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn guess_empty_dir() {
        let tmp = std::env::temp_dir().join("syncraft_test_empty");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let (ver, loader) = guess_version(tmp.clone());
        assert_eq!(ver, None);
        assert!(matches!(loader, Loader::Vanilla));
        let _ = fs::remove_dir_all(&tmp);
    }
}
