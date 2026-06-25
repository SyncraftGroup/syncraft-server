use std::fmt::Display;

use owo_colors::OwoColorize;
use syncraft_protocol::{VersionInfo, Loader};

use crate::error::Error;

pub enum MessageKey<'a> {
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
    CleanUpWorkSpaceConfirm
}

struct Prefixed<P, T> {
    pub prefix: P,
    pub value: T,
}

impl<P: Display, T: Display> Display for Prefixed<P, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:>8} {}", self.prefix, self.value)
    }
}

struct Reason<R, H> {
    pub reason: R,
    pub hint: Option<H>
}

impl<R: Display> Reason<R, String> {
    pub fn with_hint<H: Display>(self, hint: H) -> Reason<R, H> {
        Reason { reason: self.reason, hint: Some(hint) }
    }
}

impl<R: Display, H: Display> Display for Reason<R, H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", format!("Reason:\n    {}", self.reason).bright_black())?;
        if let Some(hint) = &self.hint {
            write!(f, "{}", format!("\nHint:\n    {}", hint).bright_black())?;
        }
        Ok(())
    }
}

trait MessageFormatExt: Sized + Display {
    fn with_prefix<P: Display>(self, prefix: P) -> Prefixed<P, Self> {
        Prefixed { prefix, value: self }
    }

    fn reasonify<H: Display>(self) -> Reason<Self, H> {
        Reason { reason: self, hint: None }
    }

    fn finish(self) -> String {
        format!("{}", self)
    }
}

impl<T: Display + Sized> MessageFormatExt for T {}

trait VecMessageExt<T> {
    fn finish_all(&self) -> String;
}

impl<T: Display> VecMessageExt<T> for Vec<T> {
    fn finish_all(&self) -> String {
        self.iter()
            .map(|item| item.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

pub fn message(message: MessageKey) {
    eprintln!("{}", translate(message));
}

pub fn input(prompt: MessageKey) -> Result<String, Error> {
    inquire::Text::new(&translate(prompt))
        .prompt()
        .map_err(|err| Error::UI(err.to_string()))
}

pub fn confirm(prompt: MessageKey) -> Result<bool, Error> {
    inquire::Confirm::new(&translate(prompt))
        .prompt()
        .map_err(|err| Error::UI(err.to_string()))
}

pub fn translate(key: MessageKey) -> String {
    match key {
        MessageKey::Initializing => match get_locale() {
            Locale::En => "Initializing...",
            Locale::Zh => "初始化工作区..."
        }.green().bold().finish(),
        MessageKey::Uninitialized => match get_locale() {
            Locale::En => "Cannot find valid syncraft workspace. Please run init.",
            Locale::Zh => "找不到合法的 syncraft 工作区。请执行 init 命令。"
        }.yellow().finish(),
        MessageKey::InitOutsideMinecraftServerDirectory => match get_locale() {
            Locale::En => "Initializing out of a runned Minecraft server directory",
            Locale::Zh => "当前目录不是已经运行过的 Minecraft 服务器目录"
        }.purple().reasonify().with_hint(match get_locale() {
            Locale::En => "Please run the server for at least once, then run init inside the server directory.",
            Locale::Zh => "请确保服务器运行过至少一次后，在服务器目录内执行 init 命令。"
        }.yellow()).finish(),
        MessageKey::AlreadyInitialized => match get_locale() {
            Locale::En => "Directory is already initialized.",
            Locale::Zh => "当前目录已存在工作区。"
        }.purple().reasonify().with_hint(match get_locale() {
            Locale::En => "If you're sure about this, add `--force` to dismiss the old data.",
            Locale::Zh => "如果你明白自己在做什么，添加 `--force` 来丢弃旧数据。"
        }.yellow()).finish(),
        MessageKey::InputServerName => match get_locale() {
            Locale::En => "Please enter the name of your server (e.t. MyServer):",
            Locale::Zh => "请输入服务器名："
        }.bright_cyan().finish(),
        MessageKey::InputVanillaVersion => match get_locale() {
            Locale::En => "Please enter your server's version (e.t. 1.20.1):",
            Locale::Zh => "请输入服务器版本（e.t. 1.20.1）："
        }.bright_cyan().finish(),
        MessageKey::VersionInfoDisplay(info) => match get_locale() {
            Locale::En => vec![
                "Version Info:".finish(),
                format!("name: {}", info.name).bright_green().with_prefix("-").finish(),
                format!("minecraft: {}", info.vanilla_version).bright_green().with_prefix("-").finish(),
                format!("loader: {}", match &info.loader {
                    Loader::Vanilla => "Vanilla (No ModLoader)".to_string(),
                    Loader::Fabric { loader_version } => format!("Fabric {loader_version}"),
                    Loader::Forge { loader_version } => format!("Forge {loader_version}"),
                    Loader::NeoForge { loader_version } => format!("NeoForge {loader_version}")
                }).bright_green().with_prefix("-").finish()
            ].finish_all(),
            Locale::Zh => vec![
                "版本信息：".bright_green().bold().finish(),
                format!("服务器名: {}", info.name).bright_green().with_prefix("-").finish(),
                format!("游戏版本: {}", info.vanilla_version).bright_green().with_prefix("-").finish(),
                format!("加载器: {}", match &info.loader {
                    Loader::Vanilla => "Vanilla (No ModLoader)".to_string(),
                    Loader::Fabric { loader_version } => format!("Fabric {loader_version}"),
                    Loader::Forge { loader_version } => format!("Forge {loader_version}"),
                    Loader::NeoForge { loader_version } => format!("NeoForge {loader_version}")
                }).bright_green().with_prefix("-").finish()
            ].finish_all()
        }.finish(),
        MessageKey::VersionInfoConfirm => match get_locale() {
            Locale::En => "Please confirm that your version info is correct. (y/n)",
            Locale::Zh => "请确认服务器版本信息正确。(y/n)"
        }.bright_cyan().finish(),
        MessageKey::ContinueToInitializeConfirm => match get_locale() {
            Locale::En => "Continue to initialize anyway? You can edit '/syncraft/syncraft.toml' to corrent the info. (y/n)",
            Locale::Zh => "继续初始化？你可以编辑 '/syncraft/syncraft.toml' 以修正信息。(y/n)"
        }.bright_cyan().finish(),
        MessageKey::InitializationCancelled => match get_locale() {
            Locale::En => "Initialization cancelled.",
            Locale::Zh => "已取消初始化。"
        }.yellow().finish(),
        MessageKey::InitializationFinished => match get_locale() {
            Locale::En => "Initialization finished.",
            Locale::Zh => "初始化已完成。"
        }.green().finish(),
        MessageKey::CleanUpWorkSpaceConfirm => match get_locale() {
            Locale::En => "You are removing the whole syncraft workspace. Confirm? (y/n)",
            Locale::Zh => "你正在删除整个 syncraft 工作区，你确定要这样做吗？(y/n)"
        }.bright_red().finish()
    }
}

pub enum Locale {
    Zh, En
}

pub fn get_locale() -> Locale {
    let locale = sys_locale::get_locale().unwrap_or("en".into());
    if locale.starts_with("en") { Locale::En }
    else if locale.starts_with("zh") { Locale::Zh }
    else { Locale::En }
}

#[test]
fn ui_test() -> Result<(), Error> {
    let a = input(MessageKey::InputServerName)?;
    println!("{a}");
    Ok(())
}