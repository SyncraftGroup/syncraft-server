use clap::{Parser, Subcommand};

use crate::error::Error;

mod error;
mod config;
mod ui;
mod paths;
mod init;

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
    Init,
    /// Start the Syncraft server
    Serve,
    /// Publish a new version
    Publish,
    /// Show current status
    Status,
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let cli = Cli::parse();

    match cli.command {
        Command::Init => {
        }
        Command::Serve => {
        }
        Command::Publish => {
        }
        Command::Status => {
            println!("Syncraft is installed.");
        }
    }
    Ok(())
}