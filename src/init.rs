use std::{fs, path::{Path, PathBuf}};

use git2::Repository;
use syncraft_protocol::{Loader, VersionInfo};

use crate::{config::Config, error::Error, paths::SyncraftPaths, ui::{self, MessageKey}};

pub async fn run(paths: SyncraftPaths, force: bool) -> Result<(), Error> {
    if looks_like_mc_server_dir(&paths.root())? {
        ui::message(MessageKey::Initializing);
        let paths = SyncraftPaths::new(paths.root());
        if paths.workspace().exists() {
            if force
            && ui::confirm(MessageKey::CleanUpWorkSpaceConfirm)? {
                cleanup_old_workspace(&paths).await?;
            } else {
                ui::message(MessageKey::AlreadyInitialized);
                return Ok(());
            }
        }

        let name = ui::input(MessageKey::InputServerName)?;
        let (vanilla_version, loader) = {
            let (vv, loader) = guess_version(paths.root());
            (if let Some(vanilla_version) = vv {
                vanilla_version
            } else {
                ui::input(MessageKey::InputVanillaVersion)?
            }, loader)
        };
        let version_info = VersionInfo {
            name, vanilla_version, loader
        };

        ui::message(MessageKey::VersionInfoDisplay(&version_info));
        if !ui::confirm(MessageKey::VersionInfoConfirm)?
        && !ui::confirm(MessageKey::ContinueToInitializeConfirm)? {
            ui::message(MessageKey::InitializationCancelled);
            return Ok(());
        }

        paths.create_dirs().await?;

        let config = Config {
            info: version_info,
            ..Default::default()
        };

        write_config(&paths, &config).await?;

        let repo = Repository::init(paths.root())?;
        create_gitignore(&paths).await?;
        initial_commit(&repo)?;
        
        ui::message(MessageKey::InitializationFinished);
    } else {
        ui::message(MessageKey::InitOutsideMinecraftServerDirectory);
    }

    Ok(())
}

async fn write_config(
    paths: &SyncraftPaths,
    config: &Config,
) -> Result<(), Error> {
    let content = toml::to_string_pretty(config)?;

    tokio::fs::write(
        paths.config_path(),
        content,
    ).await?;

    Ok(())
}

async fn create_gitignore(
    paths: &SyncraftPaths,
) -> Result<(), Error> {
    let content = r#"*
!/.gitignore
!/syncraft.toml

!/syncraft/
!/syncraft/**

/syncraft/logs/
/syncraft/logs/**
"#;

    tokio::fs::write(
        paths.root().join(".gitignore"),
        content,
    ).await?;

    Ok(())
}

fn initial_commit(
    repo: &Repository,
) -> Result<(), git2::Error> {
    let mut index = repo.index()?;

    index.add_all(
        ["*"].iter(),
        git2::IndexAddOption::DEFAULT,
        None,
    )?;

    index.write()?;

    let tree_id = index.write_tree()?;
    let tree = repo.find_tree(tree_id)?;

    let sig = git2::Signature::now(
        "syncraft",
        "syncraft@local",
    )?;

    repo.commit(
        Some("HEAD"),
        &sig,
        &sig,
        "Initialize workspace",
        &tree,
        &[],
    )?;

    Ok(())
}

async fn cleanup_old_workspace(
    paths: &SyncraftPaths
) -> Result<(), Error> {
    let workspace = paths.workspace();
    if workspace.exists() {
        tokio::fs::remove_dir_all(&workspace).await?;
    }

    let config = paths.config_path();
    if config.exists() {
        tokio::fs::remove_file(config).await?;
    }

    let gitignore = paths.root().join(".gitignore");
    if gitignore.exists() {
        tokio::fs::remove_file(gitignore).await?;
    }

    let git = paths.root().join(".git");
    if git.exists() {
        tokio::fs::remove_dir_all(git).await?;
    }

    Ok(())
}

fn looks_like_mc_server_dir(cwd: &Path) -> Result<bool, Error> {
    if cwd.join("server.properties").exists()
    && cwd.join("eula.txt").exists()
    && cwd.join("libraries").exists() {
        Ok(true)
    } else {
        Ok(false)
    }
}

pub fn guess_version(path: PathBuf) -> (Option<String>, Loader) {

    let lib_path = path.join("libraries");

    let neoforge_path = lib_path.join("net").join("neoforged");
    if neoforge_path.exists() {
        let info_dir = neoforge_path.join("neoforge");
        if info_dir.exists()
        && let Ok(mut entries) = fs::read_dir(info_dir) {
            while let Some(Ok(entry)) = entries.next() {
                if let Some(version_str) = entry.file_name().to_str()
                && let Some(pos_2) = version_str.rfind(".")
                && let Some(pos_1) = version_str[..pos_2].find(".")
                && let Ok(main_num) = version_str[..pos_1].parse::<usize>() {
                    let mut vanilla_version = version_str[..pos_2].to_string();
                    // neoforge
                    // 1.20.1 -> 20.1.xxx
                    // 26.1 -> 26.1.xxx
                    // todo: parse snapshot versions
                    // 26.1-snapshot-11 -> 26.1.0.0-alpha.x+snapshot-11
                    if main_num < 26 {
                        vanilla_version = format!("1.{}", vanilla_version);
                    }
                    return (
                        Some(vanilla_version),
                        Loader::NeoForge { loader_version: version_str.to_string() }
                    );
                }
            }
        }
    }

    let forge_path = lib_path.join("net").join("minecraftforge");
    if forge_path.exists() {
        let info_dir = forge_path.join("forge");
        if info_dir.exists()
        && let Ok(mut entries) = fs::read_dir(info_dir) {
            while let Some(Ok(entry)) = entries.next() {
                if let Some(version_str) = entry.file_name().to_str()
                && let Some(pos) = version_str.find("-") {
                    return (
                        Some(version_str[..pos].to_string()),
                        Loader::Forge { loader_version: version_str[pos+1..].to_string()}
                    );
                }
            }
        }
    }

    let fabric_path = lib_path.join("net").join("fabricmc");
    if fabric_path.exists() {
        let loader_info_dir = fabric_path.join("fabric-loader");
        let vanilla_info_dir = fabric_path.join("intermediary");
        let mut loader_version = None;
        let mut vanilla_version = None;
        if let Ok(mut loader_info_entries) = fs::read_dir(loader_info_dir) {
            while let Some(Ok(entry)) = loader_info_entries.next() {
                if let Some(version_str) = entry.file_name().to_str() {
                    loader_version = Some(version_str.to_string());
                    break;
                }
            }
        }
        if let Ok(mut vanilla_info_entries) = fs::read_dir(vanilla_info_dir) {
            while let Some(Ok(entry)) = vanilla_info_entries.next() {
                if let Some(version_str) = entry.file_name().to_str() {
                    vanilla_version = Some(version_str.to_string());
                    break;
                }
            }
        }
        if let Some(lv) = loader_version
        && let Some(vv) = vanilla_version {
            return (Some(vv), Loader::Fabric { loader_version: lv });
        }
    }

    (None, Loader::Vanilla)
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
        // main_num >= 26: no "1." prefix
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