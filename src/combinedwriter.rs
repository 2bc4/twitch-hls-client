use std::io::{self, Write};

use anyhow::{ensure, Result};

use crate::{player::Player, recorder::Recorder};

pub struct CombinedWriter {
    player: Option<Player>,
    recorder: Option<Recorder>,
}

impl Write for CombinedWriter {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        unimplemented!()
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        if let Some(ref mut player) = self.player {
            player.write_all(buf)?;
        }

        if let Some(ref mut recorder) = self.recorder {
            recorder.write_all(buf)?;
        }

        Ok(())
    }
}

impl CombinedWriter {
    pub fn new(player: Option<Player>, recorder: Option<Recorder>) -> Result<Self> {
        ensure!(
            player.is_some() || recorder.is_some(),
            "Player or recording must be set"
        );

        Ok(Self { player, recorder })
    }
}
