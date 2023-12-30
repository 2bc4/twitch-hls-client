use std::process::{Child, ChildStdin, Command, Stdio};

use anyhow::{Context, Result};
use log::{info, warn};

pub struct Player {
    process: Child,
}

impl Drop for Player {
    fn drop(&mut self) {
        if let Err(e) = self.process.kill() {
            warn!("Failed to kill player: {e}");
        }
    }
}

impl Player {
    pub fn spawn(path: &str, args: &str, quiet: bool) -> Result<Self> {
        info!("Opening player: {path} {args}");
        let mut command = Command::new(path);
        command.args(args.split_whitespace()).stdin(Stdio::piped());

        if quiet {
            command.stdout(Stdio::null()).stderr(Stdio::null());
        }

        Ok(Self {
            process: command.spawn().context("Failed to open player")?,
        })
    }

    pub fn spawn_and_wait(path: &str, args: &str, url: &str, quiet: bool) -> Result<()> {
        let new_args = args
            .split_whitespace()
            .map(|s| if s == "-" { url.to_owned() } else { s.to_owned() })
            .collect::<Vec<String>>()
            .join(" ");

        let mut player = Self::spawn(path, &new_args, quiet)?;
        player
            .process
            .wait()
            .context("Failed to wait for player process")?;

        Ok(())
    }

    pub fn stdin(&mut self) -> Result<ChildStdin> {
        self.process.stdin.take().context("Failed to open player stdin")
    }
}
