use std::path::{Path, PathBuf};

use syncraft_protocol::{Loader, VersionInfo};

use crate::config::Config;
use crate::error::Error;
use crate::git;
use crate::paths::SyncraftPaths;
use crate::log_debug;
use crate::log_warn;
use crate::ui::message::MessageKey;
use crate::ui::Ui;

pub async fn run(paths: SyncraftPaths, force: bool) -> Result<(), Error> {
    log_debug!("Init started (force={force})");

    let ui = Ui::get();

    if !looks_like_mc_server_dir(&paths.root())? {
        log_warn!("Not a Minecraft server directory");
        ui.error(MessageKey::InitOutsideMinecraftServerDirectory);
        return Ok(());
    }

    // ── Check for existing workspace ────────────────────────────
    if paths.workspace().exists() {
        if force && ui.confirm(MessageKey::CleanUpWorkSpaceConfirm)? {
            cleanup_old_workspace(&paths).await?;
            ui.ok(MessageKey::Done);
        } else {
            ui.error(MessageKey::AlreadyInitialized);
            return Ok(());
        }
    }

    // ── Collect info ────────────────────────────────────────────
    ui.step(MessageKey::Initializing);

    let name = ui.input(MessageKey::InputServerName)?;

    let (vanilla_version, loader) = {
        let (vv, loader) = guess_version(paths.root());
        let vanilla_version = if let Some(v) = vv {
            log_debug!("Version guessed: MC {v}, loader {loader:?}");
            v
        } else {
            ui.input(MessageKey::InputVanillaVersion)?
        };
        (vanilla_version, loader)
    };

    let version_info = VersionInfo {
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

    ui.plain(MessageKey::VersionInfoDisplay(&version_info));

    if !ui.confirm(MessageKey::VersionInfoConfirm)?
        && !ui.confirm(MessageKey::ContinueToInitializeConfirm)?
    {
        ui.warn(MessageKey::InitializationCancelled);
        return Ok(());
    }

    // ── Create directories ──────────────────────────────────────
    paths.create_dirs().await?;
    log_debug!("Workspace directories created");

    // ── Write config ────────────────────────────────────────────
    let config = Config {
        info: version_info,
        ..Default::default()
    };
    write_config(&paths, &config).await?;
    log_debug!("Config written to {}", paths.config_path().display());

    // ── Init git repo ───────────────────────────────────────────
    git::run(&paths, Some(git::GitAction::Init)).await?;

    ui.ok(MessageKey::InitializationFinished);
    log_debug!("Init completed successfully");
    Ok(())
}

// ── Helpers ──────────────────────────────────────────────────────────────

async fn write_config(paths: &SyncraftPaths, config: &Config) -> Result<(), Error> {
    let content = toml::to_string_pretty(config)?;
    tokio::fs::write(paths.config_path(), content).await?;
    Ok(())
}

async fn cleanup_old_workspace(paths: &SyncraftPaths) -> Result<(), Error> {
    let to_remove: [PathBuf; 4] = [
        paths.workspace(),
        paths.config_path(),
        paths.root().join(".gitignore"),
        paths.root().join(".git"),
    ];
    for path in &to_remove {
        if path.exists() {
            if path.is_dir() {
                tokio::fs::remove_dir_all(path).await?;
            } else {
                tokio::fs::remove_file(path).await?;
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

// ── Version guessing ─────────────────────────────────────────────────────

pub fn guess_version(path: PathBuf) -> (Option<String>, Loader) {
    let lib_path = path.join("libraries");

    // NeoForge
    let neoforge_path = lib_path.join("net").join("neoforged");
    if neoforge_path.exists() {
        let info_dir = neoforge_path.join("neoforge");
        if info_dir.exists() {
            if let Ok(entries) = std::fs::read_dir(info_dir) {
                for entry in entries.flatten() {
                    if let Some(version_str) = entry.file_name().to_str() {
                        if let Some(parsed) = parse_neoforge(version_str) {
                            return parsed;
                        }
                    }
                }
            }
        }
    }

    // Forge
    let forge_path = lib_path.join("net").join("minecraftforge");
    if forge_path.exists() {
        let info_dir = forge_path.join("forge");
        if info_dir.exists() {
            if let Ok(entries) = std::fs::read_dir(info_dir) {
                for entry in entries.flatten() {
                    if let Some(version_str) = entry.file_name().to_str() {
                        if let Some(pos) = version_str.find('-') {
                            return (
                                Some(version_str[..pos].to_string()),
                                Loader::Forge {
                                    loader_version: version_str[pos + 1..].to_string(),
                                },
                            );
                        }
                    }
                }
            }
        }
    }

    // Fabric
    let fabric_path = lib_path.join("net").join("fabricmc");
    if fabric_path.exists() {
        let mut loader_version = None;
        let mut vanilla_version = None;

        if let Ok(entries) = std::fs::read_dir(fabric_path.join("fabric-loader")) {
            for entry in entries.flatten() {
                if let Some(v) = entry.file_name().to_str() {
                    loader_version = Some(v.to_string());
                    break;
                }
            }
        }
        if let Ok(entries) = std::fs::read_dir(fabric_path.join("intermediary")) {
            for entry in entries.flatten() {
                if let Some(v) = entry.file_name().to_str() {
                    vanilla_version = Some(v.to_string());
                    break;
                }
            }
        }
        if let (Some(lv), Some(vv)) = (loader_version, vanilla_version) {
            return (Some(vv), Loader::Fabric { loader_version: lv });
        }
    }

    (None, Loader::Vanilla)
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

// ═══════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════

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
