use std::{
    fmt::{self, Display, Formatter},
    io::{self, ErrorKind::BrokenPipe, Write},
    process::{Child, ChildStdin, Command, Stdio},
};

use anyhow::{Context, Result, bail};
use log::{debug, error, info};

use super::Output;
use crate::{config::Config, http::Url};

#[derive(Debug)]
pub struct PlayerClosedError;

impl std::error::Error for PlayerClosedError {}

impl Display for PlayerClosedError {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        f.write_str("Unhandled player closed")
    }
}

pub struct Player {
    stdin: ChildStdin,
    process: Child,
}

impl Drop for Player {
    fn drop(&mut self) {
        if !Config::get().player_no_kill
            && let Err(e) = self.process.kill()
        {
            error!("Failed to kill player: {e}");
        }
    }
}

impl Output for Player {
    fn set_header(&mut self, header: &[u8]) -> io::Result<()> {
        self.stdin
            .write_all(header)
            .map_err(|e| self.handle_broken_pipe(e))
    }
}

impl Write for Player {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        unreachable!();
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.stdin
            .write_all(buf)
            .map_err(|e| self.handle_broken_pipe(e))
    }
}

impl Player {
    pub fn new() -> Result<Option<Self>> {
        let cfg = Config::get();

        let Some(path) = &cfg.player_path else {
            return Ok(None);
        };

        info!("Spawning player: {path}");
        let mut process = Self::spawn(path, &cfg.player_args, cfg.player_quiet)?;
        let stdin = process
            .stdin
            .take()
            .context("Failed to open player stdin")?;

        Ok(Some(Self { stdin, process }))
    }

    pub fn passthrough(url: Url) -> Result<()> {
        let cfg = Config::get();

        let Some(path) = &cfg.player_path else {
            bail!("No player set");
        };

        let mut args = cfg.player_args.clone();
        if let Some(arg) = args.iter_mut().find(|a| *a == "-") {
            *arg = url.into_string();
        } else {
            args.push(url.into_string());
        }

        info!("Spawning player (passthrough): {path}");
        debug!("Player args: {args:?}");
        let mut process = Self::spawn(path, &args, cfg.player_quiet)?;
        process
            .wait()
            .context("Failed to wait for player process")?;

        Ok(())
    }

    fn spawn(path: &str, args: &[String], quiet: bool) -> Result<Child> {
        let mut command = Command::new(path);
        command.args(args.iter()).stdin(Stdio::piped());

        if quiet {
            command.stdout(Stdio::null()).stderr(Stdio::null());
        }

        command.spawn().context("Failed to spawn player")
    }

    fn handle_broken_pipe(&mut self, error: io::Error) -> io::Error {
        if error.kind() == BrokenPipe {
            let _ = self.process.try_wait(); //reap pid
            return io::Error::other(PlayerClosedError);
        }

        error
    }
}
