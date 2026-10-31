use std::fs;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

use crate::config::Config;
use crate::error::Error;
use crate::log_debug;
use crate::log_info;
use crate::message::{self, MessageKey};
use crate::paths::SyncraftPaths;
use console_info::Ui;

const AUTH_LIB_JAR: &str = "authlib-injector.jar";
const DOWNLOAD_URL: &str = "https://bmclapi2.bangbang93.com/mirrors/authlib-injector";

pub enum AuthlibAction {
    Inject,
    Restore,
}

pub async fn run(paths: SyncraftPaths, action: AuthlibAction) -> Result<(), Error> {
    match action {
        AuthlibAction::Inject => cmd_inject(paths).await?,
        AuthlibAction::Restore => cmd_restore(&paths)?,
    }
    Ok(())
}

async fn cmd_inject(paths: SyncraftPaths) -> Result<(), Error> {
    let config = match Config::load(&paths) {
        Some(c) => c,
        None => {
            Ui::get().error(&message::translate(&MessageKey::Uninitialized));
            return Ok(());
        }
    };

    let base_url = config.yggdrasil_base_url();

    let jar_path = paths.root().join(AUTH_LIB_JAR);
    download_if_missing(&jar_path).await?;

    modify_or_create_script(&paths, &base_url, &jar_path).await?;
    ensure_online_mode(&paths)?;
    Ui::get().ok(&message::translate(&MessageKey::AuthlibDone));
    Ok(())
}

fn cmd_restore(paths: &SyncraftPaths) -> Result<(), Error> {
    let jar_path = paths.root().join(AUTH_LIB_JAR);
    if jar_path.exists() {
        fs::remove_file(&jar_path)?;
        log_info!("Removed authlib-injector.jar");
        Ui::get().ok(&message::translate(&MessageKey::AuthlibJarRemoved {
            path: jar_path.display().to_string(),
        }));
    }

    let scripts = find_startup_scripts(&paths.root());
    for script in &scripts {
        if let Ok(content) = fs::read_to_string(script) {
            if !content.contains("-javaagent:authlib-injector") {
                continue;
            }
        } else {
            continue;
        }

        let backup_path = script.with_extension(format!(
            "{}.bak",
            script.extension().unwrap_or_default().to_string_lossy()
        ));

        if backup_path.exists() {
            fs::copy(&backup_path, script)?;
            let _ = fs::remove_file(&backup_path);
            Ui::get().ok(&message::translate(&MessageKey::AuthlibScriptRestored {
                path: script.display().to_string(),
            }));
        } else {
            let content = fs::read_to_string(script)?;
            let restored = remove_javaagent(&content);
            fs::write(script, &restored)?;
            Ui::get().ok(&message::translate(&MessageKey::AuthlibScriptRestored {
                path: script.display().to_string(),
            }));
        }
    }

    Ok(())
}

async fn download_if_missing(jar_path: &std::path::Path) -> Result<(), Error> {
    if jar_path.exists() {
        let msg = message::translate(&MessageKey::AuthlibJarExists {
            path: jar_path.display().to_string(),
        });
        Ui::get().ok(&msg);
        return Ok(());
    }

    Ui::get().ok(&message::translate(&MessageKey::AuthlibDownloading));
    let response = reqwest::get(DOWNLOAD_URL)
        .await
        .map_err(|e| Error::Server(format!("Failed to download authlib-injector: {}", e)))?;

    if !response.status().is_success() {
        return Err(Error::Server(format!(
            "Download failed with status: {}",
            response.status()
        )));
    }

    let body = response
        .bytes()
        .await
        .map_err(|e| Error::Server(format!("Failed to read response body: {}", e)))?;

    fs::write(jar_path, &body)?;
    log_info!("Downloaded authlib-injector.jar");
    Ui::get().ok(&message::translate(&MessageKey::AuthlibDownloaded {
        path: jar_path.display().to_string(),
    }));

    Ok(())
}

async fn modify_or_create_script(
    paths: &SyncraftPaths,
    base_url: &str,
    jar_path: &std::path::Path,
) -> Result<(), Error> {
    let scripts = find_startup_scripts(&paths.root());
    if !scripts.is_empty() {
        for script in &scripts {
            let modified = modify_script(script, base_url)?;
            if modified {
                Ui::get().ok(&message::translate(&MessageKey::AuthlibScriptModified {
                    path: script.display().to_string(),
                }));
            } else {
                Ui::get().ok(&message::translate(
                    &MessageKey::AuthlibScriptAlreadyInjected {
                        path: script.display().to_string(),
                    },
                ));
            }
        }
    } else {
        create_startup_script(paths, base_url, jar_path)?;
    }

    Ok(())
}

fn find_startup_scripts(root: &std::path::Path) -> Vec<PathBuf> {
    let mut scripts = Vec::new();
    let extensions = ["sh", "bat", "cmd"];

    let Ok(entries) = fs::read_dir(root) else {
        return scripts;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase();
        let ext = path.extension().unwrap_or_default().to_string_lossy();

        if !extensions.contains(&ext.as_ref()) {
            continue;
        }
        if name.starts_with('.') {
            continue;
        }

        if let Ok(content) = fs::read_to_string(&path) {
            if content.contains("java") {
                scripts.push(path);
            }
        }
    }

    scripts
}

fn modify_script(script_path: &std::path::Path, base_url: &str) -> Result<bool, Error> {
    let content = fs::read_to_string(script_path)?;
    let javaagent_arg = format!(
        "-javaagent:{} -Dauthlibinjector.yggdrasilServer={}",
        AUTH_LIB_JAR, base_url
    );

    if content.contains("-javaagent:authlib-injector") {
        return Ok(false);
    }

    let modified = inject_javaagent(&content, &javaagent_arg);
    if modified == content {
        return Ok(false);
    }

    let backup_path = script_path.with_extension(format!(
        "{}.bak",
        script_path
            .extension()
            .unwrap_or_default()
            .to_string_lossy()
    ));
    fs::copy(script_path, &backup_path).ok();
    fs::write(script_path, &modified)?;

    log_debug!("Modified startup script: {}", script_path.display());
    Ok(true)
}

fn inject_javaagent(content: &str, javaagent_arg: &str) -> String {
    let mut result = String::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.is_empty() {
            result.push_str(line);
            result.push('\n');
            continue;
        }

        if let Some(modified) = inject_into_line(line, javaagent_arg) {
            log_debug!("Injected javaagent into line: {}", line.trim());
            result.push_str(&modified);
            result.push('\n');
        } else {
            result.push_str(line);
            result.push('\n');
        }
    }

    result
}

fn inject_into_line(line: &str, javaagent_arg: &str) -> Option<String> {
    if !line.contains("java") || line.contains("-javaagent:") {
        return None;
    }

    let trimmed = line.trim();
    let injection_point = if let Some(pos) = trimmed.find(" -jar ") {
        pos
    } else if let Some(pos) = trimmed.find(' ') {
        pos
    } else {
        return None;
    };

    let leading = &trimmed[..injection_point];
    let trailing = &trimmed[injection_point..];
    let leading_spaces = &line[..line.len() - trimmed.len()];

    Some(
        format!(
            "{}{} {} {}",
            leading_spaces, leading, javaagent_arg, trailing
        )
        .trim_end()
        .to_string(),
    )
}

fn create_startup_script(
    paths: &SyncraftPaths,
    base_url: &str,
    _jar_path: &std::path::Path,
) -> Result<(), Error> {
    Ui::get().ok(&message::translate(&MessageKey::AuthlibNoScriptFound));

    let input = Ui::get()
        .input(&message::translate(&MessageKey::AuthlibInputCommand))
        .map_err(|e| Error::Server(format!("Failed to read input: {}", e)))?;

    let javaagent_arg = format!(
        "-javaagent:{} -Dauthlibinjector.yggdrasilServer={}",
        AUTH_LIB_JAR, base_url
    );

    let modified = if input.contains("-javaagent:") {
        input
    } else if let Some(injected) = inject_into_line(&input, &javaagent_arg) {
        injected
    } else {
        Ui::get().error(&message::translate(&MessageKey::AuthlibCommandParseFailed));
        return Ok(());
    };

    let script_path = paths.root().join("start.sh");
    let mut file = File::create(&script_path)?;
    writeln!(file, "#!/bin/sh")?;
    writeln!(file)?;
    writeln!(file, "{}", modified)?;
    drop(file);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&script_path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script_path, perms)?;
    }

    Ui::get().ok(&message::translate(&MessageKey::AuthlibScriptCreated {
        path: script_path.display().to_string(),
    }));

    Ok(())
}

fn ensure_online_mode(paths: &SyncraftPaths) -> Result<(), Error> {
    let props_path = paths.root().join("server.properties");
    let content = match fs::read_to_string(&props_path) {
        Ok(c) => c,
        Err(_) => {
            Ui::get().ok(&message::translate(
                &MessageKey::AuthlibServerPropertiesNotFound,
            ));
            return Ok(());
        }
    };

    let mut found = false;
    let mut already_true = false;
    let modified: String = content
        .lines()
        .map(|line| {
            let trimmed = line.trim();
            if trimmed.starts_with("online-mode") {
                found = true;
                if trimmed.contains("=true") {
                    already_true = true;
                    return line.to_string();
                }
                return "online-mode=true".to_string();
            }
            line.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");

    let modified = if !found {
        let mut s = modified;
        if !s.is_empty() && !s.ends_with('\n') {
            s.push('\n');
        }
        s.push_str("online-mode=true\n");
        s
    } else {
        modified
    };

    if found && already_true {
        Ui::get().ok(&message::translate(
            &MessageKey::AuthlibOnlineModeAlreadyTrue,
        ));
        return Ok(());
    }

    fs::write(&props_path, &modified)?;
    log_info!("Updated server.properties: online-mode=true");
    Ui::get().ok(&message::translate(&MessageKey::AuthlibOnlineModeSet));

    Ok(())
}

fn remove_javaagent(content: &str) -> String {
    content
        .lines()
        .map(|line| {
            if line.contains("-javaagent:authlib-injector") {
                let parts: Vec<&str> = line
                    .split(' ')
                    .filter(|part| {
                        !part.contains("-javaagent:authlib-injector")
                            && !part.starts_with("-Dauthlibinjector.yggdrasilServer=")
                    })
                    .collect();
                parts.join(" ")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}
