use std::{
    io::{self, ErrorKind::BrokenPipe},
    process,
    sync::mpsc::{channel, sync_channel, Receiver, SendError, Sender, SyncSender},
    thread::Builder,
};

use anyhow::{Context, Result};
use url::Url;

use crate::{http::Request, player::Player};

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
