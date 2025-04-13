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
    file: FileArgs,
    tcp: TcpArgs,
}

impl Parse for Args {
    fn parse(&mut self, parser: &mut Parser) -> Result<()> {
        self.player.parse(parser)?;
        self.file.parse(parser)?;
        self.tcp.parse(parser)?;

        Ok(())
    }
}

pub struct Writer {
    player: Option<Player>,
    file: Option<File>,
    tcp: Option<Tcp>,
}

impl Write for Writer {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        unreachable!();
    }

    fn flush(&mut self) -> io::Result<()> {
        if let Some(file) = &mut self.file {
            file.flush()?;
        }

        debug!("Finished writing segment");
        Ok(())
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        debug_assert!(self.player.is_some() || self.file.is_some() || self.tcp.is_some());

        if let Some(player) = &mut self.player {
            match player.write_all(buf) {
                Ok(()) => (),
                Err(e) if self.file.is_some() || self.tcp.is_some() && e.kind() == Other => {
                    self.player = None;
                }
                Err(e) => return Err(e),
            }
        }

        if let Some(file) = &mut self.file {
            file.write_all(buf)?;
        }

        if let Some(tcp) = &mut self.tcp {
            tcp.write_all(buf)?;
        }

        Ok(())
    }
}

impl Writer {
    pub fn new(args: &Args) -> Result<Self> {
        let writer = Self {
            player: Player::spawn(&args.player)?,
            file: File::new(&args.file)?,
            tcp: Tcp::new(&args.tcp)?,
        };

        ensure!(
            writer.player.is_some() || writer.file.is_some() || writer.tcp.is_some(),
            "No output configured"
        );

        Ok(writer)
    }
}
