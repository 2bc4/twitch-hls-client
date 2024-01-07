use std::{
    fmt,
    io::{self, ErrorKind::BrokenPipe},
    process::{self, ChildStdin},
    sync::mpsc::{channel, sync_channel, Receiver, Sender, SyncSender},
    thread::Builder,
};

use anyhow::{Context, Result};
use log::debug;
use url::Url;

use crate::http::RawRequest;

#[derive(Debug)]
pub enum Error {
    Failed,
}

impl std::error::Error for Error {}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Failed => write!(f, "Failed to communicate with segment worker"),
        }
    }
}

pub struct Worker {
    url_tx: Sender<Url>,
    sync_rx: Receiver<()>,
}

impl Worker {
    pub fn new(pipe: ChildStdin) -> Result<Self> {
        let (url_tx, url_rx): (Sender<Url>, Receiver<Url>) = channel();
        let (sync_tx, sync_rx): (SyncSender<()>, Receiver<()>) = sync_channel(1);

        Builder::new()
            .name(String::from("Segment Worker"))
            .spawn(move || {
                if let Err(e) = Self::thread_main(&url_rx, &sync_tx, pipe) {
                    eprintln!("Worker error: {e}");
                    eprintln!("{}", e.backtrace());
                    process::exit(1);
                }
            })
            .context("Failed to spawn segment worker")?;

        Ok(Self { url_tx, sync_rx })
    }

    pub fn send(&self, url: Url) -> Result<(), Error> {
        debug!("Sending URL to worker: {url}");
        self.url_tx.send(url).or(Err(Error::Failed))
    }

    pub fn sync(&self) -> Result<(), Error> {
        self.sync_rx.recv().or(Err(Error::Failed))
    }

    fn thread_main(url_rx: &Receiver<Url>, sync_tx: &SyncSender<()>, pipe: ChildStdin) -> Result<()> {
        debug!("Starting...");
        let Ok(url) = url_rx.recv() else {
            return Ok(());
        };

        let mut request = RawRequest::get(&url, pipe)?;
        if should_exit(request.call())? {
            return Ok(());
        };

        sync_tx.send(()).context("Failed to sync from segment worker")?;
        loop {
            let Ok(url) = url_rx.recv() else {
                return Ok(());
            };
            debug!("Beginning new segment request");

            request.url(&url)?;
            if should_exit(request.call())? {
                return Ok(());
            };
        }
    }
}

fn should_exit(result: Result<()>) -> Result<bool> {
    debug!("Finished writing segment");
    match result {
        Ok(()) => Ok(false),
        Err(e) => match e.downcast_ref::<io::Error>() {
            Some(r) => match r.kind() {
                BrokenPipe => Ok(true),
                _ => Err(e),
            },
            _ => Err(e),
        },
    }
}
