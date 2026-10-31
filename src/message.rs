//! User-facing message catalog with i18n support (English / Chinese).
//!
//! Every user-visible string goes through [`MessageKey`].  The [`Ui`](super::Ui)
//! methods call [`translate`] internally — callers never pass raw strings.
//!
//! To add a message: add a variant to [`MessageKey`], then add both `En` and
//! `Zh` arms in [`translate`].

use std::{path::PathBuf, process::ExitStatus};

use console_info::msgs;
use owo_colors::OwoColorize;

use syncraft_protocol::{Loader, VersionInfo};

use crate::{daemon::sync::ModFileType, git::Change};

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
    InitRegisterHint,
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
    DaemonStarted {
        pid: u32,
    },
    DaemonStopped,
    DaemonNotRunning,
    DaemonStatus {
        pid: u32,
        uptime_secs: u64,
        servers: usize,
        manager: &'a str,
    },
    DaemonIpcError {
        reason: String,
    },
    DaemonStalePid {
        pid: String,
        pid_file: String,
    },
    DaemonNotRunningPublish,
    ServerNotRegistered,
    ServerAdded {
        name: String,
        path: String,
    },
    ServerRemoved {
        name: String,
    },
    ServerNotFound {
        key: String,
    },
    ServerListEmpty,
    ServerNotInitialized {
        path: String,
    },
    ServerNameEmpty,
    ServerAlreadyRegistered {
        path: String,
        name: String,
    },
    ServerNameTaken {
        name: String,
        path: String,
    },
    ServerInvalidPath {
        path: String,
        reason: String,
    },
    ServerRegistryError {
        reason: String,
    },
    ServerColumnName,
    ServerColumnPath,
    ServerColumnAddress,
    DaemonNotInstalled,
    DaemonInstallUnsupported,

    // Account
    AccountListEmpty,
    AccountColName,
    AccountColEmail,
    AccountColSkin,
    AccountColCreated,
    AccountTotal,
    AccountAdded {
        character_name: String,
    },
    AccountRemoved {
        character_name: String,
    },
    AccountYggdrasilNotConfigured,
    AccountCharExists {
        character_name: String,
    },
    AccountEmailExists {
        email: String,
    },
    AccountCharNotFound {
        character_name: String,
    },

    // Authlib-inject
    AuthlibDownloading,
    AuthlibJarExists {
        path: String,
    },
    AuthlibDownloaded {
        path: String,
    },
    AuthlibScriptModified {
        path: String,
    },
    AuthlibScriptAlreadyInjected {
        path: String,
    },
    AuthlibScriptCreated {
        path: String,
    },
    AuthlibNoScriptFound,
    AuthlibInputCommand,
    AuthlibCommandParseFailed,
    AuthlibDone,
    AuthlibServerPropertiesNotFound,
    AuthlibOnlineModeSet,
    AuthlibOnlineModeAlreadyTrue,
    AuthlibJarRemoved {
        path: String,
    },
    AuthlibScriptRestored {
        path: String,
    },

    // Mod Filter
    ModFilterMayTakeAWhile,
    GotModType(&'a PathBuf, &'a ModFileType),
    SelectModType(&'a PathBuf),
    SelectOptionIgnore,
    SelectOptionSync,

    // Generic
    Done,
    Failed {
        reason: String,
    },
    Text(String),
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
        MessageKey::VersionInfoDisplay(info) => format_version_info(info),
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
        MessageKey::InitRegisterHint => msgs! {
            En: "Next: register this server with the daemon using `syncraft server add .`.\nThe daemon is shared by every registered server.",
            Zh: "下一步：使用 `syncraft server add .` 将此服务器注册到守护进程。\n守护进程由所有已注册的服务器共用。"
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
                    Change::Added(path) => lines.push(format!(
                        "{}  {}",
                        "+".green().bold(),
                        path.to_string_lossy()
                    )),
                    Change::Deleted(path) => {
                        lines.push(format!("{}  {}", "-".red().bold(), path.to_string_lossy()))
                    }
                    Change::Modified(path) => lines.push(format!(
                        "{}  {}",
                        "M".yellow().bold(),
                        path.to_string_lossy()
                    )),
                    Change::Renamed { from, to } => lines.push(format!(
                        "{}  {} -> {}",
                        "R".blue().bold(),
                        from.to_string_lossy(),
                        to.to_string_lossy()
                    )),
                    Change::Ignored => (),
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
        MessageKey::DaemonStatus {
            pid,
            uptime_secs,
            servers,
            manager,
        } => {
            let label_status = msgs! { En: "Status",  Zh: "状态"  };
            let label_pid = msgs! { En: "PID",     Zh: "进程"  };
            let label_uptime = msgs! { En: "Uptime",  Zh: "运行"  };
            let label_servers = msgs! { En: "Servers", Zh: "服务器" };
            let label_mgr = msgs! { En: "Manager", Zh: "管理" };

            let running = msgs! { En: "Running", Zh: "运行中" };
            let uptime = format_uptime(*uptime_secs);

            vec![
                format!("{}: {}", label_status.italic(), running.bold()),
                format!("{}: {}", label_pid.italic(), pid.bold()),
                format!("{}: {}", label_uptime.italic(), uptime.bold()),
                format!("{}: {}", label_servers.italic(), servers.bold()),
                format!("{}: {}", label_mgr.italic(), manager.bold()),
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
        }
        MessageKey::DaemonNotRunningPublish => msgs! {
            En: "Syncraft daemon is not running.\nStart it first with 'syncraft daemon start', then try again.",
            Zh: "Syncraft 守护进程未运行。\n请先执行 'syncraft daemon start' 启动守护进程，然后重试。"
        },
        MessageKey::ServerNotRegistered => msgs! {
            En: "This server is not registered with the daemon.\nRun 'syncraft server add .' first.",
            Zh: "此服务器尚未注册到守护进程。\n请先执行 'syncraft server add .'。"
        },
        MessageKey::ServerAdded { name, path } => msgs! {
            En: format!("Registered '{}' ({}).", name, path),
            Zh: format!("已注册 '{}'（{}）。", name, path)
        },
        MessageKey::ServerRemoved { name } => msgs! {
            En: format!("Removed '{}'.", name),
            Zh: format!("已移除 '{}'。", name)
        },
        MessageKey::ServerNotFound { key } => msgs! {
            En: format!("Server '{}' is not registered.", key),
            Zh: format!("服务器 '{}' 未注册。", key)
        },
        MessageKey::ServerListEmpty => msgs! {
            En: "No servers registered. Use `syncraft server add <path>`.",
            Zh: "尚无已注册的服务器。使用 `syncraft server add <path>` 添加。"
        },
        MessageKey::ServerNotInitialized { path } => msgs! {
            En: format!("`{path}` is not a syncraft server. Run `syncraft init` there first."),
            Zh: format!("`{path}` 不是 syncraft 服务器目录。请先在其中执行 `syncraft init`。")
        },
        MessageKey::ServerNameEmpty => msgs! {
            En: "Server name must not be empty.",
            Zh: "服务器名称不能为空。"
        },
        MessageKey::ServerAlreadyRegistered { path, name } => msgs! {
            En: format!("`{path}` is already registered as '{name}'."),
            Zh: format!("`{path}` 已注册为 '{name}'。")
        },
        MessageKey::ServerNameTaken { name, path } => msgs! {
            En: format!("Server name '{name}' is already used by `{path}`."),
            Zh: format!("服务器名称 '{name}' 已被 `{path}` 使用。")
        },
        MessageKey::ServerInvalidPath { path, reason } => msgs! {
            En: format!("Cannot use `{path}`: {reason}"),
            Zh: format!("无法使用 `{path}`：{reason}")
        },
        MessageKey::ServerRegistryError { reason } => msgs! {
            En: format!("Failed to update the server registry: {reason}"),
            Zh: format!("更新服务器注册表失败：{reason}")
        },
        MessageKey::ServerColumnName => msgs! { En: "NAME", Zh: "名称" },
        MessageKey::ServerColumnPath => msgs! { En: "PATH", Zh: "路径" },
        MessageKey::ServerColumnAddress => msgs! { En: "ADDRESS", Zh: "地址" },
        MessageKey::DaemonNotInstalled => {
            let reason = msgs! {
                En: "Syncraft daemon is not installed.",
                Zh: "Syncraft 守护进程尚未安装。"
            };
            let hint = msgs! {
                En: "Run 'syncraft daemon install' first to set up the daemon as a systemd service.",
                Zh: "请先执行 'syncraft daemon install' 将守护进程安装为 systemd 服务。"
            };
            format_reason_hint(&reason, &hint)
        }
        MessageKey::DaemonInstallUnsupported => msgs! {
            En: "Service installation requires systemd (Linux).\nUse 'daemon start' for fallback daemon mode or 'daemon foreground' for foreground.",
            Zh: "服务安装需要 systemd (Linux)。\n使用 'daemon start' 以 fallback 模式运行，或 'daemon foreground' 以前台模式运行。"
        },

        // Account
        MessageKey::AccountListEmpty => msgs! {
            En: "No verified accounts.",
            Zh: "无已验证的账号。"
        },
        MessageKey::AccountColName => msgs! {
            En: "Name",
            Zh: "角色"
        },
        MessageKey::AccountColEmail => msgs! {
            En: "Email",
            Zh: "邮箱"
        },
        MessageKey::AccountColSkin => msgs! {
            En: "Skin",
            Zh: "皮肤"
        },
        MessageKey::AccountColCreated => msgs! {
            En: "Created",
            Zh: "创建时间"
        },
        MessageKey::AccountTotal => msgs! {
            En: "Total",
            Zh: "总计"
        },
        MessageKey::AccountAdded { character_name } => msgs! {
            En: format!("Account '{}' added.", character_name),
            Zh: format!("账号 '{}' 已添加。", character_name)
        },
        MessageKey::AccountRemoved { character_name } => msgs! {
            En: format!("Account '{}' removed.", character_name),
            Zh: format!("账号 '{}' 已删除。", character_name)
        },
        MessageKey::AccountYggdrasilNotConfigured => {
            let reason = msgs! {
                En: "yggdrasil.base_url is not configured in syncraft.toml.",
                Zh: "syncraft.toml 中未配置 yggdrasil.base_url。"
            };
            let hint = msgs! {
                En: "Add a [yggdrasil] section with base_url in your config file.",
                Zh: "请在配置文件中添加 [yggdrasil] 节并设置 base_url。"
            };
            format_reason_hint(&reason, &hint)
        }
        MessageKey::AccountCharExists { character_name } => msgs! {
            En: format!("Character '{}' already exists.", character_name),
            Zh: format!("角色 '{}' 已存在。", character_name)
        },
        MessageKey::AccountEmailExists { email } => msgs! {
            En: format!("Email '{}' is already registered.", email),
            Zh: format!("邮箱 '{}' 已注册。", email)
        },
        MessageKey::AccountCharNotFound { character_name } => msgs! {
            En: format!("Character '{}' not found.", character_name),
            Zh: format!("角色 '{}' 未找到。", character_name)
        },

        // Authlib-inject
        MessageKey::AuthlibDownloading => msgs! {
            En: "Downloading authlib-injector...",
            Zh: "正在下载 authlib-injector..."
        },
        MessageKey::AuthlibJarExists { path } => msgs! {
            En: format!("authlib-injector.jar already exists at '{}'.", path),
            Zh: format!("authlib-injector.jar 已存在 '{}'。", path)
        },
        MessageKey::AuthlibDownloaded { path } => msgs! {
            En: format!("Downloaded authlib-injector to '{}'.", path),
            Zh: format!("authlib-injector 已下载到 '{}'。", path)
        },
        MessageKey::AuthlibScriptModified { path } => msgs! {
            En: format!("Modified startup script '{}'. Original backed up.", path),
            Zh: format!("已修改启动脚本 '{}'。已备份原文件。", path)
        },
        MessageKey::AuthlibScriptAlreadyInjected { path } => msgs! {
            En: format!("Startup script '{}' already contains javaagent. Skipped.", path),
            Zh: format!("启动脚本 '{}' 已包含 javaagent。已跳过。", path)
        },
        MessageKey::AuthlibScriptCreated { path } => msgs! {
            En: format!("Created startup script '{}'.", path),
            Zh: format!("已生成启动脚本 '{}'。", path)
        },
        MessageKey::AuthlibNoScriptFound => msgs! {
            En: "No startup script found.",
            Zh: "未找到启动脚本。"
        },
        MessageKey::AuthlibInputCommand => msgs! {
            En: "Enter the full command you use to start the server (e.g. java -Xmx4G -jar server.jar nogui):",
            Zh: "请输入启动服务器所用的完整命令（例如 java -Xmx4G -jar server.jar nogui）："
        },
        MessageKey::AuthlibCommandParseFailed => {
            let reason = msgs! {
                En: "Failed to parse the startup command.",
                Zh: "无法解析启动命令。"
            };
            let hint = msgs! {
                En: "Make sure the command contains 'java' at the beginning.",
                Zh: "请确保命令以 'java' 开头。"
            };
            format_reason_hint(&reason, &hint)
        }
        MessageKey::AuthlibDone => msgs! {
            En: "Authlib-injector setup complete.",
            Zh: "authlib-injector 配置完成。"
        },
        MessageKey::AuthlibServerPropertiesNotFound => msgs! {
            En: "server.properties not found. Skipped online-mode check.",
            Zh: "未找到 server.properties。已跳过 online-mode 检查。"
        },
        MessageKey::AuthlibOnlineModeSet => msgs! {
            En: "Set online-mode=true in server.properties.",
            Zh: "已在 server.properties 中设置 online-mode=true。"
        },
        MessageKey::AuthlibOnlineModeAlreadyTrue => msgs! {
            En: "online-mode is already true. Skipped.",
            Zh: "online-mode 已为 true。已跳过。"
        },
        MessageKey::AuthlibJarRemoved { path } => msgs! {
            En: format!("Removed '{}'.", path),
            Zh: format!("已删除 '{}'。", path)
        },
        MessageKey::AuthlibScriptRestored { path } => msgs! {
            En: format!("Restored startup script '{}'.", path),
            Zh: format!("已还原启动脚本 '{}'。", path)
        },

        // Mod Filter
        MessageKey::ModFilterMayTakeAWhile => msgs! {
            En: "There are some mods to recognize, this may take a while...",
            Zh: "有一些需要辨别类型的 mod，这可能会花费一些时间..."
        },
        MessageKey::GotModType(path, mod_type) => {
            let type_str = match mod_type {
                ModFileType::Ignore => msgs! {
                    En: "Ignored",
                    Zh: "忽略"
                },
                ModFileType::Sync => msgs! {
                    En: "Sync",
                    Zh: "同步"
                },
                ModFileType::Unknown => "???".to_string(),
            };
            format!("{} : {}", path.to_string_lossy(), type_str)
        }
        MessageKey::SelectModType(path) => msgs! {
            En: format!("Please select for the mod below manually: \n{}", path.to_string_lossy().yellow().italic()),
            Zh: format!("请手动为以下模组选择：\n{}", path.to_string_lossy().yellow().italic())
        },
        MessageKey::SelectOptionIgnore => msgs! {
            En: "Ignore (This mod is server-only)",
            Zh: "忽略 （该模组仅在服务端运行）"
        },
        MessageKey::SelectOptionSync => msgs! {
            En: "Sync (This mod should be sent to clients)",
            Zh: "同步 （该模组应该被同步到客户端）"
        },

        // Generic
        MessageKey::Done => msgs! {
            En: "Done.",
            Zh: "完成。"
        },
        MessageKey::Failed { reason } => {
            format!("Failed: {}", reason)
        }
        MessageKey::Text(text) => text.to_string(),
    }
}

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
        format!(
            "  {}: {}",
            version_label.italic(),
            info.vanilla_version.bold()
        ),
        format!("  {}: {}", loader_label.italic(), loader_str.bold()),
    ]
    .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_does_not_panic() {
        let _ = console_info::get_locale();
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
            id: "test".into(),
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
