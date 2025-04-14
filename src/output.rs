mod file;
mod player;
mod tcp;

pub use player::Player;

use std::io::{self, ErrorKind::Other, Write};

use anyhow::{Result, ensure};
use log::debug;

use file::{Args as FileArgs, File};
use player::Args as PlayerArgs;
use tcp::{Args as TcpArgs, Tcp};

use crate::args::{Parse, Parser};

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

        if let Some(player) = &mut self.player {
            match player.write_all(buf) {
                Ok(()) => (),
                Err(e) if self.tcp.is_some() || self.file.is_some() && e.kind() == Other => {
                    self.player = None;
                }
                Err(e) => return Err(e),
            }
        }

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
}
