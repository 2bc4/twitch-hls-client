mod player;
mod recorder;

pub use player::Player;

use std::io::{self, ErrorKind::Other, Write};

use anyhow::{Result, bail};
use log::debug;

use player::Args as PlayerArgs;
use recorder::{Args as RecorderArgs, Recorder};

use crate::args::{Parse, Parser};

#[derive(Default, Debug)]
pub struct Args {
    pub player: PlayerArgs,
    recorder: RecorderArgs,
}

impl Parse for Args {
    fn parse(&mut self, parser: &mut Parser) -> Result<()> {
        self.player.parse(parser)?;
        self.recorder.parse(parser)?;

        Ok(())
    }
}

pub enum Writer {
    Player(Player),
    Recorder(Recorder),
    Combined(Player, Recorder),
}

impl Write for Writer {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        unreachable!();
    }

    fn flush(&mut self) -> io::Result<()> {
        debug!("Finished writing segment");
        match self {
            Self::Player(_) => Ok(()),
            Self::Recorder(recorder) | Self::Combined(_, recorder) => recorder.flush(),
        }
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        match self {
            Self::Player(player) => player.write_all(buf),
            Self::Recorder(recorder) => recorder.write_all(buf),
            Self::Combined(player, recorder) => {
                if let Err(e) = player.write_all(buf) {
                    match e.kind() {
                        Other => (), //ignore player closed
                        _ => return Err(e),
                    }
                }

                recorder.write_all(buf)?;
                Ok(())
            }
        }
    }
}

impl Writer {
    pub fn new(args: &Args) -> Result<Self> {
        match (Player::spawn(&args.player)?, Recorder::new(&args.recorder)?) {
            (Some(player), Some(recorder)) => Ok(Self::Combined(player, recorder)),
            (Some(player), None) => Ok(Self::Player(player)),
            (None, Some(recorder)) => Ok(Self::Recorder(recorder)),
            (None, None) => bail!("Player or recording must be set"),
        }
    }
}
