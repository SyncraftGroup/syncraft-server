mod config;
mod daemon;
mod error;
mod init;
mod git;
mod log;
mod paths;
mod ui;
mod yggdrasil;

use clap::{Parser, Subcommand};

use crate::daemon::ServeAction;
use crate::error::Error;
use crate::git::GitAction;
use crate::paths::SyncraftPaths;
use crate::ui::Ui;

#[derive(Parser)]
#[command(
    version,
    about = "Syncraft Minecraft Synchronization Server"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Initialize a workspace
    Init {
        /// Delete the old workspace if one exists
        #[arg(long)]
        force: bool,
    },
    /// Manage the syncraft server daemon.
    ///
    /// Without a subcommand, defaults to `start`.
    Serve {
        #[command(subcommand)]
        action: Option<ServeCommand>,
    },
    /// Git commands
    Git {
        #[command(subcommand)]
        action: Option<GitCommand>
    },
    /// Publish a new version
    Publish,
    /// Show current status
    Status,
}

#[derive(Subcommand, Clone, Copy, Debug, PartialEq, Eq)]
enum ServeCommand {
    /// Start the server daemon
    Start,
    /// Stop a running daemon
    Stop,
    /// Query daemon status
    Status,
    /// Run the server in the foreground (development / debug)
    Run,
    /// Install as a platform service (systemd user unit on Linux)
    Install,
    /// Remove the platform service
    Uninstall,
    /// Tail daemon logs (like `docker logs -f`)
    Logs
}

impl From<ServeCommand> for ServeAction {
    fn from(cmd: ServeCommand) -> Self {
        match cmd {
            ServeCommand::Start => ServeAction::Start,
            ServeCommand::Stop => ServeAction::Stop,
            ServeCommand::Status => ServeAction::Status,
            ServeCommand::Run => ServeAction::Run,
            ServeCommand::Install => ServeAction::Install,
            ServeCommand::Uninstall => ServeAction::Uninstall,
            ServeCommand::Logs => ServeAction::Logs,
        }
    }
}

#[derive(Subcommand, Clone, Copy, Debug, PartialEq, Eq)]
enum GitCommand {
    /// Initialize a repository.
    Init,
    /// Get current tracking status.
    Status,
    /// Publish a new update.
    Publish
}

impl From<GitCommand> for GitAction {
    fn from(cmd: GitCommand) -> Self {
        match cmd {
            GitCommand::Init => GitAction::Init,
            GitCommand::Status => GitAction::Status,
            GitCommand::Publish => GitAction::Publish
        }
    }
}

fn main() -> Result<(), Error> {
    log::init();
    ui::configure_inquire();

    let cli = Cli::parse();
    let cwd = std::env::current_dir()?;
    log_debug!("Working directory: {}", cwd.display());
    let paths = SyncraftPaths::new(cwd);

    match cli.command {
        Command::Init { force } => {
            init::run(paths, force)?;
        }
        Command::Serve { action } => {
            let action: Option<ServeAction> = action.map(Into::into);
            daemon::handle_command(&paths, action.as_ref())?;
        }
        Command::Git { action } => {
            let action: Option<GitAction> = action.map(Into::into);
            git::run(paths, action)?;
        }
        Command::Publish => {
            git::run(paths, Some(GitAction::Publish))?;
        }
        Command::Status => {
            let ui = Ui::get();
            ui.info(ui::message::MessageKey::Done);
        }
    }

    Ok(())
}
