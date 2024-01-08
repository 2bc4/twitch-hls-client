use std::process::{Child, ChildStdin, Command, Stdio};

use anyhow::{Context, Result};
use log::{error, info};
use url::Url;

pub struct Player {
    process: Child,
    no_kill: bool,
}

impl Drop for Player {
    fn drop(&mut self) {
        if !self.no_kill {
            if let Err(e) = self.process.kill() {
                error!("Failed to kill player: {e}");
            }
        }
    }
}

impl Player {
    pub fn spawn(path: &str, args: &str, quiet: bool, no_kill: bool) -> Result<Self> {
        info!("Opening player: {path} {args}");
        let mut command = Command::new(path);
        command.args(args.split_whitespace()).stdin(Stdio::piped());

        if quiet {
            command.stdout(Stdio::null()).stderr(Stdio::null());
        }

        Ok(Self {
            process: command.spawn().context("Failed to open player")?,
            no_kill,
        })
    }

    pub fn passthrough(path: &str, args: &str, quiet: bool, no_kill: bool, url: &Url) -> Result<()> {
        info!("Passing through playlist URL to player");
        let args = args
            .split_whitespace()
            .map(|s| if s == "-" { url.to_string() } else { s.to_owned() })
            .collect::<Vec<String>>()
            .join(" ");

        let mut player = Self::spawn(path, &args, quiet, no_kill)?;
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
