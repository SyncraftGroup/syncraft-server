mod account;
mod authlib;
mod config;
mod daemon;
mod error;
mod git;
mod init;
mod log;
mod message;
mod paths;
mod server;
mod yggdrasil;

use clap::{Parser, Subcommand};

use crate::account::AccountAction;
use crate::authlib::AuthlibAction;
use crate::daemon::DaemonAction;
use crate::error::Error;
use crate::git::GitAction;
use crate::paths::{GlobalPaths, SyncraftPaths};

#[derive(Parser)]
#[command(
    name = "syncraft",
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
    /// Manage the syncraft daemon (independent of any single server)
    Daemon {
        #[command(subcommand)]
        action: DaemonCommand,
    },
    /// Register / list Minecraft server roots managed by the daemon
    Server {
        #[command(subcommand)]
        action: ServerCommand,
    },
    /// Git commands
    Git {
        #[command(subcommand)]
        action: Option<GitCommand>,
    },
    /// Publish a new version
    Publish,
    /// Show current status
    Status,
    /// Manage user accounts
    Account {
        #[command(subcommand)]
        action: AccountCommand,
    },
    /// Install authlib-injector and patch startup scripts
    Authlib {
        #[command(subcommand)]
        action: AuthlibCommand,
    },
}

#[derive(Subcommand, Clone, Copy, Debug, PartialEq, Eq)]
enum DaemonCommand {
    /// Start the daemon in the background (systemd or fallback double-fork)
    Start,
    /// Stop the running daemon
    Stop,
    /// Show daemon and registered server status
    Status,
    /// Run the daemon in the foreground (systemd ExecStart / debugging)
    Foreground,
    /// Install as a platform service (systemd user unit on Linux)
    Install,
    /// Remove the platform service
    Uninstall,
    /// Tail daemon logs (like `docker logs -f`)
    Logs,
}

impl From<DaemonCommand> for DaemonAction {
    fn from(cmd: DaemonCommand) -> Self {
        match cmd {
            DaemonCommand::Start => DaemonAction::Start,
            DaemonCommand::Stop => DaemonAction::Stop,
            DaemonCommand::Status => DaemonAction::Status,
            DaemonCommand::Foreground => DaemonAction::Foreground,
            DaemonCommand::Install => DaemonAction::Install,
            DaemonCommand::Uninstall => DaemonAction::Uninstall,
            DaemonCommand::Logs => DaemonAction::Logs,
        }
    }
}

#[derive(Subcommand, Clone, Debug, PartialEq, Eq)]
enum ServerCommand {
    /// Register a Minecraft server directory (must be initialized first)
    Add {
        /// Server directory (defaults to the current directory)
        #[arg(default_value = ".")]
        path: std::path::PathBuf,
        /// Tenant name (defaults to `info.id` from syncraft.toml)
        #[arg(long)]
        name: Option<String>,
    },
    /// Unregister a server by name or path
    Remove { server: String },
    /// List registered servers
    List,
}

#[derive(Subcommand, Clone, Copy, Debug, PartialEq, Eq)]
enum GitCommand {
    /// Initialize a repository.
    Init,
    /// Get current tracking status.
    Status,
    /// Publish a new update.
    Publish,
}

impl From<GitCommand> for GitAction {
    fn from(cmd: GitCommand) -> Self {
        match cmd {
            GitCommand::Init => GitAction::Init,
            GitCommand::Status => GitAction::Status,
            GitCommand::Publish => GitAction::Publish,
        }
    }
}

#[derive(Subcommand, Clone, Debug, PartialEq, Eq)]
enum AccountCommand {
    /// List all verified accounts
    List,
    /// Add a verified account
    Add {
        /// Email address
        email: String,
        /// Character name
        character_name: String,
        /// Password
        password: String,
    },
    /// Remove an account
    Remove {
        /// Character name
        character_name: String,
    },
}

impl From<AccountCommand> for AccountAction {
    fn from(cmd: AccountCommand) -> Self {
        match cmd {
            AccountCommand::List => AccountAction::List,
            AccountCommand::Add {
                email,
                character_name,
                password,
            } => AccountAction::Add {
                email,
                character_name,
                password,
            },
            AccountCommand::Remove { character_name } => AccountAction::Remove { character_name },
        }
    }
}

#[derive(Subcommand, Clone, Copy, Debug, PartialEq, Eq)]
enum AuthlibCommand {
    /// Install authlib-injector and patch startup scripts
    Inject,
    /// Remove authlib-injector and restore original scripts
    Restore,
}

impl From<AuthlibCommand> for AuthlibAction {
    fn from(cmd: AuthlibCommand) -> Self {
        match cmd {
            AuthlibCommand::Inject => AuthlibAction::Inject,
            AuthlibCommand::Restore => AuthlibAction::Restore,
        }
    }
}

fn main() -> Result<(), Error> {
    log::init();
    console_info::configure_inquire();

    let cli = Cli::parse();
    let cwd = std::env::current_dir()?;
    log_debug!("Working directory: {}", cwd.display());
    let paths = SyncraftPaths::new(cwd);
    let global = GlobalPaths::discover();

    match cli.command {
        Command::Init { force } => {
            init::run(paths, force)?;
        }
        Command::Daemon { action } => {
            let action: DaemonAction = action.into();
            daemon::handle_command(&global, &action)?;
        }
        Command::Server { action } => {
            let action = match action {
                ServerCommand::Add { path, name } => server::ServerAction::Add { path, name },
                ServerCommand::Remove { server } => server::ServerAction::Remove { key: server },
                ServerCommand::List => server::ServerAction::List,
            };
            server::run(&global, action)?;
        }
        Command::Git { action } => {
            let action: Option<GitAction> = action.map(Into::into);
            git::run(paths, action)?;
        }
        Command::Publish => {
            git::run(paths, Some(GitAction::Publish))?;
        }
        Command::Status => {
            daemon::handle_command(&global, &DaemonAction::Status)?;
        }
        Command::Account { action } => {
            let action: AccountAction = action.into();
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| Error::Daemon(format!("Failed to create runtime: {}", e)))?;
            rt.block_on(account::run(paths, action))?;
        }
        Command::Authlib { action } => {
            let action: AuthlibAction = action.into();
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| Error::Daemon(format!("Failed to create runtime: {}", e)))?;
            rt.block_on(authlib::run(paths, action))?;
        }
    }

    Ok(())
}
