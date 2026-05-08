//! MineShare daemon — M0.
//!
//! Subcommands:
//!   (default) / `run`      Run the daemon (mDNS announce + browse + Noise XX handshake)
//!   `collect [--push]`     Bundle recent logs + system info into `logs/<hostname>.log`,
//!                          optionally git-add/commit/push to the current repo.

mod clipboard;
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
    Run {
        /// Don't capture local mouse/keyboard input (peer-receive only).
        #[arg(long)]
        no_capture: bool,
        /// Don't inject events received from peers (capture-only diagnostic).
        #[arg(long)]
        no_inject: bool,
    },
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
    match cli.command.unwrap_or(Command::Run {
        no_capture: false,
        no_inject: false,
    }) {
        Command::Run {
            no_capture,
            no_inject,
        } => {
            runtime::run(runtime::RunOpts {
                capture: !no_capture,
                inject: !no_inject,
            })
            .await
        }
        Command::Collect { push } => collect::run(push),
    }
}
