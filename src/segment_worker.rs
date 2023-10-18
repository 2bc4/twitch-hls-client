use std::{
    fmt,
    io::{self, ErrorKind::BrokenPipe},
    process::{self, ChildStdin},
    sync::{
        mpsc::{channel, sync_channel, Receiver, Sender, SyncSender},
        Arc, Mutex,
    },
    thread::Builder,
};

use anyhow::{Context, Result};
use url::Url;

use crate::http::Request;

#[derive(Debug)]
pub enum Error {
    SendFailed,
    SyncFailed,
}

impl std::error::Error for Error {}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::SendFailed => write!(f, "Failed to send URL to segment worker"),
            Self::SyncFailed => write!(f, "Failed to recieve sync state from segment worker"),
        }
    }
}

pub struct Worker {
    url_tx: Sender<Url>,
    sync_rx: Receiver<()>,
}

impl Worker {
    pub fn new(pipe: Arc<Mutex<ChildStdin>>) -> Result<Self> {
        let (url_tx, url_rx): (Sender<Url>, Receiver<Url>) = channel();
        let (sync_tx, sync_rx): (SyncSender<()>, Receiver<()>) = sync_channel(1);

        Builder::new()
            .name(String::from("Segment Worker"))
            .spawn(move || {
                // :(

                if let Err(e) = Self::thread_main(&url_rx, &sync_tx, &pipe) {
                    eprintln!("Error: {e}");
                    process::exit(1);
                }
            })
            .context("Failed to spawn segment worker thread")?;

        Ok(Self { url_tx, sync_rx })
    }

    pub fn send(&self, url: Url) -> Result<(), Error> {
        self.url_tx.send(url).or(Err(Error::SendFailed))
    }

    pub fn sync(&self) -> Result<(), Error> {
        self.sync_rx.recv().or(Err(Error::SyncFailed))
    }

    fn thread_main(
        url_rx: &Receiver<Url>,
        sync_tx: &SyncSender<()>,
        pipe: &Arc<Mutex<ChildStdin>>,
    ) -> Result<()> {
        let mut request = match url_rx.recv() {
            Ok(url) => {
                let mut request = Request::get(url)?;
                if let Err(e) = io::copy(&mut request.reader()?, &mut *pipe.lock().unwrap()) {
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
            .context("Failed to send sync state from segment worker")?;

        loop {
            let Ok(url) = url_rx.recv() else {
                return Ok(());
            };

            request.set_url(url)?;

            if let Err(e) = io::copy(&mut request.reader()?, &mut *pipe.lock().unwrap()) {
                match e.kind() {
                    BrokenPipe => return Ok(()),
                    _ => return Err(e.into()),
                }
            }
        }
    }
}
