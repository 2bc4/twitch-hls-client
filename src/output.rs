mod player;
mod recorder;

pub use player::Player;

use std::io::{self, ErrorKind::Other, Write};

use anyhow::{Result, ensure};
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

pub struct Writer {
    player: Option<Player>,
    recorder: Option<Recorder>,
}

impl Write for Writer {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        unreachable!();
    }

    fn flush(&mut self) -> io::Result<()> {
        if let Some(recorder) = &mut self.recorder {
            recorder.flush()?;
        }

        debug!("Finished writing segment");
        Ok(())
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        debug_assert!(self.player.is_some() || self.recorder.is_some());

        if let Some(player) = &mut self.player {
            match player.write_all(buf) {
                Ok(()) => (),
                Err(e) if self.recorder.is_some() && e.kind() == Other => self.player = None,
                Err(e) => return Err(e),
            }
        }

        if let Some(recorder) = &mut self.recorder {
            recorder.write_all(buf)?;
        }

        Ok(())
    }
}

impl Writer {
    pub fn new(args: &Args) -> Result<Self> {
        let writer = Self {
            player: Player::spawn(&args.player)?,
            recorder: Recorder::new(&args.recorder)?,
        };

        ensure!(
            writer.player.is_some() || writer.recorder.is_some(),
            "No output configured"
        );

        Ok(writer)
    }
}
