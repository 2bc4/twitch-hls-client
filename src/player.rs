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

    pub fn stdin(&mut self) -> Arc<Mutex<ChildStdin>> {
        self.stdin.clone()
    }

    pub fn wait(&mut self) -> Result<ExitStatus> {
        Ok(self.process.wait()?)
    }
}
