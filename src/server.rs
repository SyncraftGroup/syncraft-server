//! `syncraft server` — register Minecraft server roots with the daemon.

use std::path::PathBuf;

use console_info::Ui;
use tabled::builder::Builder;
use tabled::settings::Style;

use crate::daemon::ipc::{IpcRequest, IpcResponse, is_daemon_alive, send_request};
use crate::daemon::registry::{Registry, RegistryError};
use crate::error::Error;
use crate::log_warn;
use crate::message::{MessageKey, translate};
use crate::paths::GlobalPaths;

pub enum ServerAction {
    Add { path: PathBuf, name: Option<String> },
    Remove { key: String },
    List,
}

pub fn run(global: &GlobalPaths, action: ServerAction) -> Result<(), Error> {
    let mut registry = match Registry::load(global) {
        Ok(registry) => registry,
        Err(e) => {
            registry_error(&e);
            return Ok(());
        }
    };

    match action {
        ServerAction::Add { path, name } => match registry.add(&path, name) {
            Ok(entry) => {
                if !save(global, &registry) {
                    return Ok(());
                }
                notify_daemon(global);
                Ui::get().ok(&translate(&MessageKey::ServerAdded {
                    name: entry.name,
                    path: entry.path.display().to_string(),
                }));
            }
            Err(e) => registry_error(&e),
        },
        ServerAction::Remove { key } => match registry.remove(&key) {
            Some(removed) => {
                if !save(global, &registry) {
                    return Ok(());
                }
                notify_daemon(global);
                Ui::get().ok(&translate(&MessageKey::ServerRemoved {
                    name: removed.name,
                }));
            }
            None => {
                Ui::get().error(&translate(&MessageKey::ServerNotFound { key }));
            }
        },
        ServerAction::List => {
            if let Err(e) = list(global, &registry) {
                Ui::get().error(&translate(&MessageKey::Failed {
                    reason: e.to_string(),
                }));
            }
        }
    }

    Ok(())
}

fn save(global: &GlobalPaths, registry: &Registry) -> bool {
    match registry.save(global) {
        Ok(()) => true,
        Err(e) => {
            registry_error(&e);
            false
        }
    }
}

fn registry_error(error: &RegistryError) {
    let message = match error {
        RegistryError::NotInitialized(path) => translate(&MessageKey::ServerNotInitialized {
            path: path.display().to_string(),
        }),
        RegistryError::EmptyName => translate(&MessageKey::ServerNameEmpty),
        RegistryError::DuplicatePath { path, name } => {
            translate(&MessageKey::ServerAlreadyRegistered {
                path: path.display().to_string(),
                name: name.clone(),
            })
        }
        RegistryError::DuplicateName { name, path } => translate(&MessageKey::ServerNameTaken {
            name: name.clone(),
            path: path.display().to_string(),
        }),
        RegistryError::InvalidPath { path, reason } => translate(&MessageKey::ServerInvalidPath {
            path: path.display().to_string(),
            reason: reason.clone(),
        }),
        RegistryError::Io(_) | RegistryError::TomlDe(_) | RegistryError::TomlSer(_) => {
            translate(&MessageKey::ServerRegistryError {
                reason: error.to_string(),
            })
        }
    };
    Ui::get().error(&message);
}

/// Ask a running daemon to reload its registry. Silently skipped when the
/// daemon is not running — the change applies on next start anyway.
fn notify_daemon(global: &GlobalPaths) {
    if !is_daemon_alive(&global.socket_path(), &global.pid_file()) {
        return;
    }

    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            log_warn!("Failed to create runtime for daemon reload: {e}");
            return;
        }
    };

    match rt.block_on(send_request(&global.socket_path(), &IpcRequest::Reload)) {
        Ok(IpcResponse::Ok) => {}
        Ok(IpcResponse::Error { message }) => {
            log_warn!("Daemon rejected registry reload: {message}");
        }
        Err(e) => log_warn!("Failed to reload daemon: {e}"),
        _ => {}
    }
}

fn list(global: &GlobalPaths, registry: &Registry) -> Result<(), Error> {
    if registry.servers.is_empty() {
        Ui::get().echo(&translate(&MessageKey::ServerListEmpty));
        return Ok(());
    }

    let statuses = if is_daemon_alive(&global.socket_path(), &global.pid_file()) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| Error::Daemon(format!("Failed to create runtime: {e}")))?;
        match rt.block_on(send_request(&global.socket_path(), &IpcRequest::Status)) {
            Ok(IpcResponse::Status { servers, .. }) => servers,
            _ => Vec::new(),
        }
    } else {
        Vec::new()
    };

    let mut builder = Builder::new();
    builder.push_record([
        translate(&MessageKey::ServerColumnName),
        translate(&MessageKey::ServerColumnPath),
        translate(&MessageKey::ServerColumnAddress),
    ]);

    for entry in &registry.servers {
        let address = statuses
            .iter()
            .find(|status| status.name == entry.name)
            .map(|status| status.address.clone())
            .unwrap_or_else(|| "-".into());
        let path = entry.path.display().to_string();
        builder.push_record([entry.name.clone(), path, address]);
    }

    let mut table = builder.build();
    table.with(Style::modern());
    Ui::get().echo(&table.to_string());
    Ok(())
}
