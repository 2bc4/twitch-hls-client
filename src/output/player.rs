use std::{
    borrow::Cow,
    fmt::{self, Display, Formatter},
    io::{self, ErrorKind::BrokenPipe, Write},
    process::{Child, ChildStdin, Command, Stdio},
};

use anyhow::{Context, Result, bail};
use log::{error, info};

use crate::args::{Parse, Parser};

#[derive(Debug)]
pub struct PipeClosedError;

impl std::error::Error for PipeClosedError {}

impl Display for PipeClosedError {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "Unhandled player closed")
    }
}

#[derive(Clone, Debug)]
pub struct Args {
    path: Option<String>,
    pargs: Cow<'static, str>,
    quiet: bool,
    no_kill: bool,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            pargs: "-".into(),
            path: Option::default(),
            quiet: bool::default(),
            no_kill: bool::default(),
        }
    }
}

impl Parse for Args {
    fn parse(&mut self, parser: &mut Parser) -> Result<()> {
        parser.parse_opt_string_cfg(&mut self.path, "-p", "player")?;
        parser.parse_cow_string_cfg(&mut self.pargs, "-a", "player-args")?;
        parser.parse_switch_or(&mut self.quiet, "-q", "--quiet")?;
        parser.parse_switch(&mut self.no_kill, "--no-kill")?;

        Ok(())
    }
}

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
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        unreachable!();
    }

    fn flush(&mut self) -> io::Result<()> {
        unreachable!();
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.stdin.write_all(buf).map_err(|error| {
            if error.kind() == BrokenPipe {
                let _ = self.process.try_wait(); //reap pid
                return io::Error::other(PipeClosedError);
            }

            error
        })
    }
}

impl Player {
    pub fn spawn(args: &Args) -> Result<Option<Self>> {
        let Some(path) = &args.path else {
            return Ok(None);
        };

        info!("Opening player: {path} {}", args.pargs);
        let mut command = Command::new(path);
        command
            .args(args.pargs.split_whitespace())
            .stdin(Stdio::piped());

        if args.quiet {
            command.stdout(Stdio::null()).stderr(Stdio::null());
        }

        let mut process = command.spawn().context("Failed to open player")?;
        let stdin = process
            .stdin
            .take()
            .context("Failed to open player stdin")?;

        Ok(Some(Self {
            stdin,
            process,
            no_kill: args.no_kill,
        }))
    }

    pub fn passthrough(args: &mut Args, url: &str) -> Result<()> {
        info!("Passing through playlist URL to player");
        if args.pargs.split_whitespace().any(|a| a == "-") {
            args.pargs = args
                .pargs
                .split_whitespace()
                .map(|a| {
                    if a == "-" {
                        url.to_owned()
                    } else {
                        a.to_owned()
                    }
                })
                .collect::<Vec<String>>()
                .join(" ")
                .into();
        } else {
            args.pargs = format!("{} {url}", args.pargs).into();
        }

        let Some(mut player) = Self::spawn(args)? else {
            bail!("No player set");
        };

        player
            .process
            .wait()
            .context("Failed to wait for player process")?;

        Ok(())
    }
}
