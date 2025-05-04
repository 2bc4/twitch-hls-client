mod file;
mod player;
mod tcp;

pub use player::{Player, PlayerClosedError};

use std::io::{self, ErrorKind::Other, Write};

use anyhow::{Result, ensure};
use log::{debug, info};

use file::{Args as FileArgs, File};
use player::Args as PlayerArgs;
use tcp::{Args as TcpArgs, Tcp};

use crate::args::{Parse, Parser};

pub trait Output {
    fn set_header(&mut self, header: &[u8]) -> io::Result<()>;

    fn should_wait(&self) -> bool {
        unreachable!();
    }

    fn wait_for_output(&mut self) -> io::Result<()> {
        unreachable!();
    }
}

#[derive(Default, Debug)]
pub struct Args {
    pub player: PlayerArgs,
    tcp: TcpArgs,
    file: FileArgs,
}

impl Parse for Args {
    fn parse(&mut self, parser: &mut Parser) -> Result<()> {
        self.player.parse(parser)?;
        self.tcp.parse(parser)?;
        self.file.parse(parser)?;

        Ok(())
    }
}

pub struct Writer {
    player: Option<Player>,
    tcp: Option<Tcp>,
    file: Option<File>,
}

impl Output for Writer {
    fn set_header(&mut self, header: &[u8]) -> io::Result<()> {
        debug!("Outputting segment header");
        self.handle_player(|player| player.set_header(header))?;

        if let Some(tcp) = &mut self.tcp {
            tcp.set_header(header)?;
        }

        if let Some(file) = &mut self.file {
            file.set_header(header)?;
        }

        Ok(())
    }

    fn should_wait(&self) -> bool {
        match (&self.player, &self.tcp, &self.file) {
            (None, Some(tcp), None) => tcp.should_wait(),
            _ => false,
        }
    }

    fn wait_for_output(&mut self) -> io::Result<()> {
        debug_assert!(self.tcp.is_some() && self.player.is_none() && self.file.is_none());

        info!("Waiting for outputs...");
        self.tcp
            .as_mut()
            .expect("Missing TCP output while waiting for output")
            .wait_for_output()?;

        Ok(())
    }
}

impl Write for Writer {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        unreachable!();
    }

    fn flush(&mut self) -> io::Result<()> {
        if let Some(tcp) = &mut self.tcp {
            tcp.flush()?;
        }

        if let Some(file) = &mut self.file {
            file.flush()?;
        }

        debug!("Finished writing segment");
        Ok(())
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        debug_assert!(self.player.is_some() || self.tcp.is_some() || self.file.is_some());

        self.handle_player(|player| player.write_all(buf))?;

        if let Some(tcp) = &mut self.tcp {
            tcp.write_all(buf)?;
        }

        if let Some(file) = &mut self.file {
            file.write_all(buf)?;
        }

        Ok(())
    }
}

impl Writer {
    pub fn new(args: &Args) -> Result<Self> {
        let writer = Self {
            player: Player::spawn(&args.player)?,
            tcp: Tcp::new(&args.tcp)?,
            file: File::new(&args.file)?,
        };

        ensure!(
            writer.player.is_some() || writer.tcp.is_some() || writer.file.is_some(),
            "No output configured"
        );

        Ok(writer)
    }

    fn handle_player<F>(&mut self, f: F) -> io::Result<()>
    where
        F: FnOnce(&mut Player) -> io::Result<()>,
    {
        if let Some(player) = &mut self.player {
            if let Err(e) = f(player) {
                if e.kind() == Other && self.tcp.is_some() || self.file.is_some() {
                    self.player = None;
                    return Ok(());
                }

                return Err(e);
            }
        }

        Ok(())
    }
}
