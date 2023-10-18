use std::{
    path::PathBuf,
    process::{Child, ChildStdin, Command, ExitStatus, Stdio},
};

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
    pub fn spawn(path: &PathBuf, args: &str) -> Result<Self> {
        info!("Opening player: {} {}", path.display(), args);
        Ok(Self {
            process: Command::new(path)
                .args(args.split_whitespace())
                .stdin(Stdio::piped())
                .spawn()
                .context("Failed to open player")?,
        })
    }

    pub fn stdin(&mut self) -> Result<ChildStdin> {
        self.process
            .stdin
            .take()
            .context("Failed to open player stdin")
    }

    pub fn wait(&mut self) -> Result<ExitStatus> {
        Ok(self.process.wait()?)
    }
}
