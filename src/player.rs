use std::{
    io::{self, Write},
    process::{Child, ChildStdin, Command, Stdio},
};

use anyhow::{Context, Result};
use log::{error, info};
use url::Url;

pub struct Player {
    stdin: ChildStdin,
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

impl Write for Player {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.stdin.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stdin.flush()
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.stdin.write_all(buf)
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

        let mut process = command.spawn().context("Failed to open player")?;
        let stdin = process
            .stdin
            .take()
            .context("Failed to open player stdin")?;

        Ok(Self {
            stdin,
            process,
            no_kill,
        })
    }

    pub fn passthrough(
        path: &str,
        args: &str,
        quiet: bool,
        no_kill: bool,
        url: &Url,
    ) -> Result<()> {
        info!("Passing through playlist URL to player");
        let args = args
            .split_whitespace()
            .map(|s| {
                if s == "-" {
                    url.to_string()
                } else {
                    s.to_owned()
                }
            })
            .collect::<Vec<String>>()
            .join(" ");

        let mut player = Self::spawn(path, &args, quiet, no_kill)?;
        player
            .process
            .wait()
            .context("Failed to wait for player process")?;

        Ok(())
    }
}
