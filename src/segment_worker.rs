//    Copyright (C) 2023 2bc4
//
//    This program is free software: you can redistribute it and/or modify
//    it under the terms of the GNU General Public License as published by
//    the Free Software Foundation, either version 3 of the License, or
//    (at your option) any later version.
//
//    This program is distributed in the hope that it will be useful,
//    but WITHOUT ANY WARRANTY; without even the implied warranty of
//    MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
//    GNU General Public License for more details.
//
//    You should have received a copy of the GNU General Public License
//    along with this program.  If not, see <https://www.gnu.org/licenses/>.

use std::{
    io,
    io::{Error, ErrorKind, ErrorKind::BrokenPipe, Write},
    process,
    process::{Command, Stdio},
    thread::Builder,
};

use anyhow::{ensure, Context, Result};
use crossbeam_channel::{bounded, unbounded, Receiver, Sender};
use is_terminal::IsTerminal;
use log::info;

pub struct Worker {
    ready_rx: Receiver<()>,
    segment_tx: Sender<Vec<u8>>,
}

impl Write for Worker {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        //Acts as a buffer if the player isn't consuming stdin quickly enough.
        //If the player is paused it will buffer endlessly until OOM.
        //Perhaps it should be limited in some way.
        self.segment_tx.send(buf.to_vec()).or(Err(Error::new(
            ErrorKind::Other,
            "Failed to send segment data to segment worker thread",
        )))?;

        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Worker {
    pub fn new(player_path: &Option<String>, player_args: &str) -> Result<Self> {
        let (ready_tx, ready_rx): (Sender<()>, Receiver<()>) = bounded(1);
        let (segment_tx, segment_rx): (Sender<Vec<u8>>, Receiver<Vec<u8>>) = unbounded();
        let path = player_path.clone();
        let args = player_args.to_owned();

        Builder::new()
            .name("Segment Worker".to_owned())
            .spawn(move || {
                // :(

                if let Err(e) = Self::thread_main(&ready_tx, &segment_rx, path, &args) {
                    eprintln!("Error: {e}");
                    process::exit(1);
                }
            })
            .context("Failed to spawn segment worker thread")?;

        Ok(Self {
            ready_rx,
            segment_tx,
        })
    }

    pub fn wait_until_ready(&self) -> Result<()> {
        self.ready_rx
            .recv()
            .context("Failed to receive ready state from segment worker thread")
    }

    fn thread_main(
        ready_tx: &Sender<()>,
        segment_rx: &Receiver<Vec<u8>>,
        player_path: Option<String>,
        player_args: &str,
    ) -> Result<()> {
        let mut pipe: Box<dyn Write> = if let Some(player_path) = player_path {
            info!("Opening player: {} {}", player_path, player_args);
            Box::new(
                Command::new(player_path)
                    .args(player_args.split_whitespace())
                    .stdin(Stdio::piped())
                    .spawn()
                    .context("Failed to open player")?
                    .stdin
                    .take()
                    .context("Failed to open player stdin")?,
            )
        } else {
            ensure!(
                !io::stdout().is_terminal(),
                "No player set and stdout is a terminal, exiting..."
            );

            info!("Writing to stdout");
            Box::new(io::stdout().lock())
        };

        ready_tx
            .send(())
            .context("Failed to send ready status from segment worker thread")?;

        loop {
            let Ok(buf) = segment_rx.recv() else { return Ok(()); };

            if let Err(e) = io::copy(&mut buf.as_slice(), &mut pipe) {
                match e.kind() {
                    BrokenPipe => {
                        info!("Pipe closed, exiting...");
                        process::exit(0);
                    }
                    _ => return Err(e.into()),
                }
            }
        }
    }
}
