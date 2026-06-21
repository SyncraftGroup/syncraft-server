use axum::Router;
use clap::{Parser, Subcommand};
use tokio::net::TcpListener;

use crate::{error::Error, paths::SyncraftPaths};

mod error;
mod config;
mod ui;
mod paths;
mod init;
mod yggdrasil;

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
        force: bool
    },
    /// Start the syncraft server
    Serve,
    /// Publish a new version
    Publish,
    /// Show current status
    Status,
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let cli = Cli::parse();
    let cwd = std::env::current_dir()?;
    let paths = SyncraftPaths::new(cwd);

    match cli.command {
        Command::Init { force } => {
            init::run(paths, force).await?;
        }
        Command::Serve => {
            let app = Router::new()
                .nest("/yggdrasil", yggdrasil::init(paths).await?);

            let listener = TcpListener::bind("0.0.0.0:3000").await?;
            axum::serve(listener, app).await?;
        }
        Command::Publish => {
        }
        Command::Status => {
            println!("Syncraft is installed.");
        }
    }
    Ok(())
}