mod config;
mod daemon;
mod error;
mod init;
mod log;
mod paths;
mod ui;
mod yggdrasil;

use clap::{Parser, Subcommand};

use crate::daemon::ServeAction;
use crate::error::Error;
use crate::paths::SyncraftPaths;
use crate::ui::Ui;

// ── CLI ──────────────────────────────────────────────────────────────────

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
    /// Publish a new version
    Publish,
    /// Show current status
    Status,
}

// ── Serve subcommands ────────────────────────────────────────────────────

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
    Logs,
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

// ── Entry point ──────────────────────────────────────────────────────────

fn main() -> Result<(), Error> {
    // Initialise the logger and inquire styling as early as possible.
    log::init();
    ui::configure_inquire();

    let cli = Cli::parse();
    let cwd = std::env::current_dir()?;
    log_debug!("Working directory: {}", cwd.display());
    let paths = SyncraftPaths::new(cwd);

    match cli.command {
        Command::Serve { action } => {
            let action: Option<ServeAction> = action.map(Into::into);
            daemon::handle_command(&paths, action.as_ref())?;
        }
        Command::Init { force } => {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .map_err(|e| Error::Daemon(format!("Failed to create runtime: {}", e)))?;
            rt.block_on(init::run(paths, force))?;
        }
        Command::Publish => {
            // TODO: implement publish
        }
        Command::Status => {
            let ui = Ui::get();
            ui.info(ui::message::MessageKey::Done);
        }
    }

    Ok(())
}
