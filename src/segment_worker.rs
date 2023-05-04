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
    io::{ErrorKind::BrokenPipe, Read, Write},
    process,
    process::{Command, Stdio},
    sync::mpsc::{channel, sync_channel, Receiver, Sender, SyncSender},
    thread::Builder,
};

use anyhow::{ensure, Context, Result};
use is_terminal::IsTerminal;
use log::info;

use crate::http::Request;

pub struct Worker {
    url_tx: Sender<String>,
    sync_rx: Receiver<()>,
}

impl Worker {
    pub fn new(player_path: &Option<String>, player_args: &str) -> Result<Self> {
        let (url_tx, url_rx): (Sender<String>, Receiver<String>) = channel();
        let (sync_tx, sync_rx): (SyncSender<()>, Receiver<()>) = sync_channel(1);
        let path = player_path.clone();
        let args = player_args.to_owned();

        Builder::new()
            .name("Segment Worker".to_owned())
            .spawn(move || {
                // :(

                if let Err(e) = Self::thread_main(&url_rx, &sync_tx, path, &args) {
                    eprintln!("Error: {e}");
                    process::exit(1);
                }
            })
            .context("Failed to spawn segment worker thread")?;

        Ok(Self { url_tx, sync_rx })
    }

    pub fn send(&self, url: &str) -> Result<()> {
        self.url_tx
            .send(url.to_owned())
            .context("Failed to send URL to segment reader thread")
    }

    pub fn sync(&self) -> Result<()> {
        self.sync_rx
            .recv()
            .context("Failed to receive ready state from segment worker thread")
    }

    fn thread_main(
        url_rx: &Receiver<String>,
        sync_tx: &SyncSender<()>,
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

        let mut request = Request::get(&url_rx.recv()?)?;
        copy_segment(&mut request.reader()?, &mut pipe)?;

        sync_tx
            .send(())
            .context("Failed to send ready status from segment worker thread")?;

        loop {
            let Ok(url) = url_rx.recv() else { return Ok(()); };
            request.set_url(&url)?;

            copy_segment(&mut request.reader()?, &mut pipe)?;
        }
    }
}

#[inline]
fn copy_segment(reader: &mut impl Read, writer: &mut impl Write) -> Result<()> {
    match io::copy(reader, writer) {
        Ok(_) => Ok(()),
        Err(e) => match e.kind() {
            BrokenPipe => {
                info!("Pipe closed, exiting...");
                process::exit(0);
            }
            _ => Err(e.into()),
        },
    }
}
