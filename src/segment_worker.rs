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
    io::{self, ErrorKind::BrokenPipe},
    path::PathBuf,
    process::{self, Child, ChildStdin, Command, ExitStatus, Stdio},
    sync::mpsc::{channel, sync_channel, Receiver, SendError, Sender, SyncSender},
    thread::Builder,
};

use anyhow::{Context, Result};
use log::{info, warn};
use url::Url;

use crate::http::Request;

pub struct Player {
    process: Child,
}

impl Drop for Player {
    fn drop(&mut self) {
        if let Err(e) = self.process.kill() {
            warn!("Failed to kill player: {e}");
        }
    }
}

impl Player {
    pub fn spawn(path: &PathBuf, args: &str) -> Result<Self> {
        info!("Opening player: {} {}", path.display(), args);
        Ok(Self {
            process: Command::new(path)
                .args(args.split_whitespace())
                .stdin(Stdio::piped())
                .spawn()
                .context("Failed to open player")?,
        })
    }

    pub fn stdin(&mut self) -> Result<ChildStdin> {
        self.process
            .stdin
            .take()
            .context("Failed to open player stdin")
    }

    pub fn wait(&mut self) -> Result<ExitStatus> {
        Ok(self.process.wait()?)
    }
}

pub struct Worker {
    url_tx: Sender<Url>,
    sync_rx: Receiver<()>,
}

impl Worker {
    pub fn new(player: Player) -> Result<Self> {
        let (url_tx, url_rx): (Sender<Url>, Receiver<Url>) = channel();
        let (sync_tx, sync_rx): (SyncSender<()>, Receiver<()>) = sync_channel(1);

        Builder::new()
            .name(String::from("Segment Worker"))
            .spawn(move || {
                // :(

                if let Err(e) = Self::thread_main(&url_rx, &sync_tx, player) {
                    eprintln!("Error: {e}");
                    process::exit(1);
                }
            })
            .context("Failed to spawn segment worker thread")?;

        Ok(Self { url_tx, sync_rx })
    }

    pub fn send(&self, url: Url) -> Result<(), SendError<Url>> {
        self.url_tx.send(url)
    }

    pub fn sync(&self) -> Result<()> {
        self.sync_rx
            .recv()
            .context("Failed to receive sync state from segment worker thread")
    }

    fn thread_main(
        url_rx: &Receiver<Url>,
        sync_tx: &SyncSender<()>,
        mut player: Player,
    ) -> Result<()> {
        let mut pipe = player.stdin()?;

        let mut request = match url_rx.recv() {
            Ok(url) => {
                let mut request = Request::get(url)?;
                if let Err(e) = io::copy(&mut request.reader()?, &mut pipe) {
                    match e.kind() {
                        BrokenPipe => return Ok(()),
                        _ => return Err(e.into()),
                    }
                }

                request
            }
            _ => return Ok(()),
        };

        sync_tx
            .send(())
            .context("Failed to send sync state from segment worker thread")?;

        loop {
            let Ok(url) = url_rx.recv() else {
                return Ok(());
            };
            request.set_url(url)?;

            if let Err(e) = io::copy(&mut request.reader()?, &mut pipe) {
                match e.kind() {
                    BrokenPipe => return Ok(()),
                    _ => return Err(e.into()),
                }
            }
        }
    }
}
