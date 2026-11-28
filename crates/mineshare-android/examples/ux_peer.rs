//! Interactive, host-only UX test peer. Uses a NEW private directory outside
//! the repository and never reads an installed MineShare identity.
//!
//! cargo run --locked -p mineshare-android --example ux_peer -- <new-directory> [display-name]
//! Send one Engine.command JSON object per stdin line; {"type":"exit"} stops.
//! Stdout is JSONL: ready/public Engine events/command_error/stopped. Private
//! identity key material stays in the caller's private configuration directory.

use anyhow::{Context, Result, ensure};
use mineshare_android::Engine;
use serde_json::json;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, TryRecvError};
use std::time::Duration;

enum Input {
    Command(String),
    Closed,
}

fn emit(output: &mut impl Write, value: &serde_json::Value) -> Result<()> {
    serde_json::to_writer(&mut *output, value)?;
    writeln!(output)?;
    output.flush()?;
    Ok(())
}

fn fresh_directory(requested: &Path) -> Result<PathBuf> {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .context("workspace root unavailable")?
        .canonicalize()?;
    let absolute = std::path::absolute(requested)?;
    // Resolve the existing parent before creating anything. The test directory
    // itself must not exist, preventing accidental loading of saved identities.
    let parent = absolute.parent().context("configuration directory needs a parent")?
        .canonicalize().context("create the private parent directory first")?;
    let target = parent.join(absolute.file_name().context("invalid directory name")?);
    ensure!(!target.starts_with(&workspace), "use a private directory outside the repository");
    ensure!(!target.exists(), "test configuration directory must be new; choose another name");
    std::fs::create_dir(&target)?;
    Ok(target)
}

fn main() -> Result<()> {
    ensure!(!cfg!(target_os = "android"), "ux_peer runs on the host only");
    let mut arguments = std::env::args_os().skip(1);
    let directory = arguments.next().context("usage: ux_peer <new-private-directory-outside-repo> [display-name]")?;
    let name = arguments.next().map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "MineShare UX test peer".to_owned());
    ensure!(arguments.next().is_none(), "unexpected argument");
    let directory = fresh_directory(Path::new(&directory))?;
    let engine = Engine::start(&directory, &name, 1920, 1080)?;
    let (sender, commands) = mpsc::sync_channel(64);
    std::thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if !line.trim().is_empty() && sender.send(Input::Command(line)).is_err() { return; }
        }
        let _ = sender.send(Input::Closed);
    });
    let stdout = io::stdout();
    let mut output = stdout.lock();
    emit(&mut output, &json!({"type":"ready","device_id":engine.device_id(),"port":engine.port(),"display_name":name}))?;
    let mut running = true;
    while running {
        for _ in 0..64 {
            match commands.try_recv() {
                Ok(Input::Command(line)) => {
                    let result = (|| -> Result<()> {
                        ensure!(line.len() <= 65_500, "command too large");
                        let value: serde_json::Value = serde_json::from_str(&line)?;
                        if value["type"] == "exit" { running = false; return Ok(()) }
                        engine.command(&line)
                    })();
                    if let Err(error) = result {
                        emit(&mut output, &json!({"type":"command_error","message":error.to_string()}))?;
                    }
                }
                Ok(Input::Closed) | Err(TryRecvError::Disconnected) => { running = false; break }
                Err(TryRecvError::Empty) => break,
            }
        }
        for _ in 0..256 {
            let Some(event) = engine.poll() else { break };
            let event: serde_json::Value = serde_json::from_str(&event)?;
            emit(&mut output, &event)?;
        }
        if running { std::thread::sleep(Duration::from_millis(20)); }
    }
    drop(engine);
    emit(&mut output, &json!({"type":"stopped"}))
}
