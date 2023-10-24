use std::{
    path::PathBuf,
    process::{Child, ChildStdin, Command, ExitStatus, Stdio},
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result};
use log::{info, warn};

pub struct Player {
    stdin: Arc<Mutex<ChildStdin>>,
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
    pub fn spawn(path: &PathBuf, args: &str) -> Result<Self> {
        info!("Opening player: {} {}", path.display(), args);
        let mut process = Command::new(path)
            .args(args.split_whitespace())
            .stdin(Stdio::piped())
            .spawn()
            .context("Failed to open player")?;

        Ok(Self {
            stdin: Arc::new(Mutex::new(
                process.stdin.take().context("Failed to open player stdin")?,
            )),
            process,
        })
    }

    pub fn spawn_and_wait(path: &PathBuf, args: &str, url: &str) -> Result<()> {
        let new_args = args
            .split_whitespace()
            .map(|s| if s == "-" { url.to_owned() } else { s.to_owned() })
            .collect::<Vec<String>>()
            .join(" ");

        let mut player = Self::spawn(path, &new_args)?;
        player.wait()?;
        Ok(())
    }

    pub fn stdin(&mut self) -> Arc<Mutex<ChildStdin>> {
        self.stdin.clone()
    }

    pub fn wait(&mut self) -> Result<ExitStatus> {
        Ok(self.process.wait()?)
    }
}