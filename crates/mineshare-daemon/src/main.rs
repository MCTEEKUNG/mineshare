//! MineShare daemon — M0.
//!
//! Subcommands:
//!   (default) / `run`      Run the daemon (mDNS announce + browse + Noise XX handshake)
//!   `collect [--push]`     Bundle recent logs + system info into `logs/<hostname>.log`,
//!                          optionally git-add/commit/push to the current repo.

mod collect;
mod identity;
mod logs;
mod runtime;

use anyhow::Result;
use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "mineshare-daemon",
    version,
    about = "MineShare KVM-over-IP daemon"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(clap::Subcommand, Debug)]
enum Command {
    /// Run the daemon. Default if no subcommand is given.
    Run,
    /// Bundle recent log files + system info for sharing.
    Collect {
        /// After writing logs/<hostname>.log, run `git add/commit/push` in cwd.
        #[arg(long)]
        push: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::Run) {
        Command::Run => runtime::run().await,
        Command::Collect { push } => collect::run(push),
    }
}
