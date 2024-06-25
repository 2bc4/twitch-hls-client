mod player;
mod recorder;

pub use player::Player;

use std::io::{self, ErrorKind::Other, Write};

use anyhow::{bail, Result};
use log::debug;

use player::Args as PlayerArgs;
use recorder::{Args as RecorderArgs, Recorder};

use crate::args::{ArgParser, Parser};

#[derive(Default, Debug)]
pub struct Args {
    pub player: PlayerArgs,
    recorder: RecorderArgs,
}

impl ArgParser for Args {
    fn parse(&mut self, parser: &mut Parser) -> Result<()> {
        self.player.parse(parser)?;
        self.recorder.parse(parser)?;

        Ok(())
    }
}

pub enum OutputWriter {
    Player(Player),
    Recorder(Recorder),
    Combined(Player, Recorder),
}

impl Write for OutputWriter {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        unimplemented!()
    }

    fn flush(&mut self) -> io::Result<()> {
        debug!("Finished writing segment");
        Ok(())
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

impl OutputWriter {
    pub fn new(args: &Args) -> Result<Self> {
        match (Player::spawn(&args.player)?, Recorder::new(&args.recorder)?) {
            (Some(player), Some(recorder)) => Ok(Self::Combined(player, recorder)),
            (Some(player), None) => Ok(Self::Player(player)),
            (None, Some(recorder)) => Ok(Self::Recorder(recorder)),
            (None, None) => bail!("Player or recording must be set"),
        }
    }
}
