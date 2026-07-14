//! User-facing message catalog with i18n support (English / Chinese).
//!
//! Every user-visible string goes through [`MessageKey`].  The [`Ui`](super::Ui)
//! methods call [`translate`] internally — callers never pass raw strings.
//!
//! To add a message: add a variant to [`MessageKey`], then add both `En` and
//! `Zh` arms in [`translate`].

use std::process::ExitStatus;

use owo_colors::OwoColorize;

use syncraft_protocol::{Loader, VersionInfo};

use crate::{error::Error, git::Change};

/// Every user-facing message is identified by a variant of this enum.
///
/// Parameters (like server name, version, etc.) are passed as fields.
/// The actual text is produced by [`translate`], which handles locale
/// selection and any rich formatting (hints, nested bullets, etc.).
#[derive(Debug)]
pub enum MessageKey<'a> {
    // Init
    Initializing,
    Uninitialized,
    InitOutsideMinecraftServerDirectory,
    AlreadyInitialized,
    InputServerName,
    InputVanillaVersion,
    VersionInfoDisplay(&'a VersionInfo),
    VersionInfoConfirm,
    ContinueToInitializeConfirm,
    InitializationCancelled,
    InitializationFinished,
    CleanUpWorkSpaceConfirm,

    // Git
    Changes(Vec<Change>),
    FailedToLaunchEditor(String),
    EditorExitedUnexpectedly(String, ExitStatus),
    PublishAbortedEmptyUpdate,
    PublishAbortedEmptyMessage,
    PublishConfirm,
    PublishCancelled,
    Published(String),

    // Daemon
    DaemonAlreadyRunning,
    DaemonStarted { pid: u32 },
    DaemonStopped,
    DaemonNotRunning,
    DaemonStatus {
        pid: u32,
        uptime_secs: u64,
        address: String,
        manager: &'a str,
    },
    DaemonIpcError { reason: String },
    DaemonStalePid { pid: String, pid_file: String },
    DaemonNotRunningPublish,
    DaemonNotInstalled,
    DaemonInstallUnsupported,

    // Generic
    Done,
    Failed { reason: String },
    Text(String)
}

pub fn translate(key: &MessageKey) -> String {
    match key {
        // Init
        MessageKey::Initializing => msgs! {
            En: "Initializing workspace...",
            Zh: "初始化工作区..."
        },
        MessageKey::Uninitialized => msgs! {
            En: "Cannot find valid syncraft workspace.\nRun 'syncraft init' first.",
            Zh: "找不到合法的 syncraft 工作区。\n请先执行 'syncraft init'。"
        },
        MessageKey::InitOutsideMinecraftServerDirectory => {
            let reason = msgs! {
                En: "Current directory does not look like a Minecraft server directory.",
                Zh: "当前目录不是已运行过的 Minecraft 服务器目录。"
            };
            let hint = msgs! {
                En: "Run the Minecraft server at least once, then run 'syncraft init' inside the server directory.",
                Zh: "请确保 Minecraft 服务器运行过至少一次，然后在服务器目录内执行 'syncraft init'。"
            };
            format_reason_hint(&reason, &hint)
        }
        MessageKey::AlreadyInitialized => {
            let reason = msgs! {
                En: "Directory is already initialized.",
                Zh: "当前目录已存在工作区。"
            };
            let hint = msgs! {
                En: "Use --force to discard the old workspace and re-initialize.",
                Zh: "使用 --force 丢弃旧数据并重新初始化。"
            };
            format_reason_hint(&reason, &hint)
        }
        MessageKey::InputServerName => msgs! {
            En: "Server name (e.g. MyServer):",
            Zh: "服务器名称（例如 MyServer）："
        },
        MessageKey::InputVanillaVersion => msgs! {
            En: "Minecraft version (e.g. 1.20.1):",
            Zh: "Minecraft 版本（例如 1.20.1）："
        },
        MessageKey::VersionInfoDisplay(info) => {
            format_version_info(info)
        }
        MessageKey::VersionInfoConfirm => msgs! {
            En: "Confirm version info (y/n)?",
            Zh: "确认版本信息正确 (y/n)？"
        },
        MessageKey::ContinueToInitializeConfirm => msgs! {
            En: "Continue anyway? You can edit syncraft/syncraft.toml later. (y/n)",
            Zh: "仍然继续？之后可以编辑 syncraft/syncraft.toml 修正。(y/n)"
        },
        MessageKey::InitializationCancelled => msgs! {
            En: "Initialization cancelled.",
            Zh: "已取消初始化。"
        },
        MessageKey::InitializationFinished => msgs! {
            En: "Workspace initialized successfully.",
            Zh: "工作区初始化完成。"
        },
        MessageKey::CleanUpWorkSpaceConfirm => msgs! {
            En: "This will DELETE the entire syncraft workspace. Confirm? (y/n)",
            Zh: "这会删除整个 syncraft 工作区，确定吗？(y/n)"
        },

        // Git
        MessageKey::Changes(changes) => {

            if changes.is_empty() {
                return msgs! {
                    En: "No changes.",
                    Zh: "无更改。"
                };
            }

            let mut lines = Vec::<String>::new();
            for change in changes {
                match change {
                    Change::Added(path) => lines.push(format!("{}  {}", "+".green().bold(), path.to_string_lossy())),
                    Change::Deleted(path) => lines.push(format!("{}  {}", "-".red().bold(), path.to_string_lossy())),
                    Change::Modified(path) => lines.push(format!("{}  {}", "M".yellow().bold(), path.to_string_lossy())),
                    Change::Renamed { from, to } => lines.push(
                        format!("{}  {} -> {}", "R".blue().bold(), from.to_string_lossy(), to.to_string_lossy())
                    ),
                    Change::Ignored => ()
                }
            }
            lines.join("\n")
        }
        MessageKey::FailedToLaunchEditor(editor) => {
            let reason = msgs! {
                En: format!("Failed to launch editor '{}'.", editor),
                Zh: format!("无法打开编辑器 '{}'。", editor)
            };
            let hint = msgs! {
                En: "Set the SYNCRAFT_EDITOR environment variable to a valid executable.",
                Zh: "请在环境变量 SYNCRAFT_EDITOR 中配置可用的编辑器。"
            };
            format_reason_hint(&reason, &hint)
        }
        MessageKey::EditorExitedUnexpectedly(editor, status) => {
            format!("Editor '{}' exited with status {}.", editor, status)
        }
        MessageKey::PublishAbortedEmptyUpdate => {
            msgs! {
                En: "Publish aborted (an empty update).",
                Zh: "未发布更新（未检测到更新）。"
            }
        }
        MessageKey::PublishAbortedEmptyMessage => {
            msgs! {
                En: "Publish aborted (empty update message).",
                Zh: "未发布更新（空更新信息）。"
            }
        }
        MessageKey::PublishConfirm => {
            msgs! {
                En: "Confirm to publish? (y/n)",
                Zh: "确认发布吗？(y/n)"
            }
        }
        MessageKey::PublishCancelled => {
            msgs! {
                En: "Publishing cancelled.",
                Zh: "已取消发布更新。"
            }
        }
        MessageKey::Published(hash) => {
            msgs! {
                En: format!("Published an update with hash '{}'.", hash),
                Zh: format!("已发布更新：'{}'", hash)
            }
        }

        // Daemon
        MessageKey::DaemonAlreadyRunning => msgs! {
            En: "Syncraft daemon is already running.",
            Zh: "Syncraft 守护进程已在运行中。"
        },
        MessageKey::DaemonStarted { pid } => msgs! {
            En: format!("Daemon started (PID {}).", pid),
            Zh: format!("守护进程已启动 (PID {})。", pid)
        },
        MessageKey::DaemonStopped => msgs! {
            En: "Daemon stopped.",
            Zh: "守护进程已停止。"
        },
        MessageKey::DaemonNotRunning => msgs! {
            En: "Syncraft daemon is not running.",
            Zh: "Syncraft 守护进程未运行。"
        },
        MessageKey::DaemonStatus { pid, uptime_secs, address, manager } => {
            let label_status  = msgs! { En: "Status",  Zh: "状态"  };
            let label_pid     = msgs! { En: "PID",     Zh: "进程"  };
            let label_uptime  = msgs! { En: "Uptime",  Zh: "运行"  };
            let label_addr    = msgs! { En: "Address", Zh: "地址"  };
            let label_mgr     = msgs! { En: "Manager", Zh: "管理" };

            let running = msgs! { En: "Running", Zh: "运行中" };
            let uptime = format_uptime(*uptime_secs);

            vec![
                format!("{}: {}", label_status.italic(), running.bold()),
                format!("{}: {}", label_pid.italic(),    pid.bold()),
                format!("{}: {}", label_uptime.italic(), uptime.bold()),
                format!("{}: {}", label_addr.italic(),   address.bold()),
                format!("{}: {}", label_mgr.italic(),    manager.bold()),
            ]
            .join("\n")
        }
        MessageKey::DaemonIpcError { reason } => {
            format!("Failed to contact daemon: {}", reason)
        }
        MessageKey::DaemonStalePid { pid, pid_file } => {
            format!(
                "A stale PID file exists (PID {}).\nRemove with: rm {}",
                pid, pid_file
            )
        },
        MessageKey::DaemonNotRunningPublish => msgs! {
            En: "Syncraft daemon is not running.\nStart it first with 'syncraft serve start', then try again.",
            Zh: "Syncraft 守护进程未运行。\n请先执行 'syncraft serve start' 启动守护进程，然后重试。"
        },
        MessageKey::DaemonNotInstalled => {
            let reason = msgs! {
                En: "Syncraft daemon is not installed.",
                Zh: "Syncraft 守护进程尚未安装。"
            };
            let hint = msgs! {
                En: "Run 'syncraft serve install' first to set up the daemon as a systemd service.",
                Zh: "请先执行 'syncraft serve install' 将守护进程安装为 systemd 服务。"
            };
            format_reason_hint(&reason, &hint)
        },
        MessageKey::DaemonInstallUnsupported => msgs! {
            En: "Service installation requires systemd (Linux).\nUse 'serve start' for fallback daemon mode or 'serve run' for foreground.",
            Zh: "服务安装需要 systemd (Linux)。\n使用 'serve start' 以 fallback 模式运行，或 'serve run' 以前台模式运行。"
        },

        // Generic
        MessageKey::Done => msgs! {
            En: "Done.",
            Zh: "完成。"
        },
        MessageKey::Failed { reason } => {
            format!("Failed: {}", reason)
        }
        MessageKey::Text(text) => text.to_string()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Locale {
    En,
    Zh
}

pub fn get_locale() -> Locale {
    let locale = sys_locale::get_locale().unwrap_or_else(|| "en".into());
    if locale.starts_with("zh") {
        Locale::Zh
    } else {
        Locale::En
    }
}

/// Shorthand for a two-locale match.
///
/// ```ignore
/// msgs! { En: "Hello" Zh: "你好" },
/// ```
macro_rules! msgs {
    (En: $en:expr, Zh: $zh:expr $(,)?) => {{
        match $crate::ui::message::get_locale() {
            $crate::ui::message::Locale::En => ($en).to_string(),
            $crate::ui::message::Locale::Zh => ($zh).to_string(),
        }
    }};
}
pub(crate) use msgs;

/// Format a reason+hint pair with a `→` arrow.
fn format_reason_hint(reason: &str, hint: &str) -> String {
    format!("{}\n  → {}", reason, hint)
}

/// Format a duration in seconds into a human-readable string.
fn format_uptime(secs: u64) -> String {
    let d = secs / 86400;
    let h = (secs % 86400) / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;

    let mut parts: Vec<String> = Vec::new();
    if d > 0 {
        let label = if d == 1 { "day" } else { "days" };
        parts.push(format!("{} {}", d, label));
    }
    if h > 0 {
        let label = if h == 1 { "hour" } else { "hours" };
        parts.push(format!("{} {}", h, label));
    }
    if m > 0 {
        let label = if m == 1 { "minute" } else { "minutes" };
        parts.push(format!("{} {}", m, label));
    }
    if s > 0 || parts.is_empty() {
        let label = if s == 1 { "second" } else { "seconds" };
        parts.push(format!("{} {}", s, label));
    }
    parts.join(", ")
}

/// Format a `VersionInfo` into a human-readable bullet list.
fn format_version_info(info: &VersionInfo) -> String {
    let name_label = msgs! { En: "Name", Zh: "名称" };
    let version_label = msgs! { En: "Minecraft", Zh: "游戏版本" };
    let loader_label = msgs! { En: "Loader", Zh: "加载器" };

    let loader_str = match &info.loader {
        Loader::Vanilla => msgs! {
            En: "Vanilla (No ModLoader)",
            Zh: "Vanilla (无 ModLoader)"
        },
        Loader::Fabric { loader_version } => {
            format!("Fabric {}", loader_version)
        }
        Loader::Forge { loader_version } => {
            format!("Forge {}", loader_version)
        }
        Loader::NeoForge { loader_version } => {
            format!("NeoForge {}", loader_version)
        }
    };

    vec![
        format!("  {}: {}", name_label.italic(), info.name.bold()),
        format!("  {}: {}", version_label.italic(), info.vanilla_version.bold()),
        format!("  {}: {}", loader_label.italic(), loader_str.bold()),
    ]
    .join("\n")
}

/// Prompt the user for text input.
pub fn prompt_input(prompt: &str) -> Result<String, Error> {
    inquire::Text::new(prompt)
        .prompt()
        .map_err(|e| Error::UI(e.to_string()))
}

/// Prompt the user for y/n confirmation.
pub fn prompt_confirm(prompt: &str) -> Result<bool, Error> {
    inquire::Confirm::new(prompt)
        .prompt()
        .map_err(|e| Error::UI(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_does_not_panic() {
        let _ = get_locale();
    }

    #[test]
    fn translate_simple_keys_non_empty() {
        let keys: &[MessageKey] = &[
            MessageKey::Initializing,
            MessageKey::Uninitialized,
            MessageKey::InputServerName,
            MessageKey::InputVanillaVersion,
            MessageKey::VersionInfoConfirm,
            MessageKey::ContinueToInitializeConfirm,
            MessageKey::InitializationCancelled,
            MessageKey::InitializationFinished,
            MessageKey::CleanUpWorkSpaceConfirm,
            MessageKey::DaemonAlreadyRunning,
            MessageKey::DaemonStopped,
            MessageKey::DaemonNotRunning,
            MessageKey::Done,
        ];
        for k in keys {
            let text = translate(k);
            assert!(!text.is_empty(), "empty translation for {:?}", k);
        }
    }

    #[test]
    fn version_info_contains_data() {
        let info = VersionInfo {
            name: "MyServer".into(),
            vanilla_version: "1.21".into(),
            loader: Loader::Fabric {
                loader_version: "0.16.0".into(),
            },
        };
        let text = translate(&MessageKey::VersionInfoDisplay(&info));
        assert!(text.contains("MyServer"));
        assert!(text.contains("1.21"));
        assert!(text.contains("Fabric"));
    }
}
