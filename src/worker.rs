use std::{
    error::Error as StdError,
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

use crate::CLIENT;

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
            Self::SyncFailed => write!(f, "Failed to sync to segment worker"),
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
                if let Err(e) = Self::thread_main(&url_rx, &sync_tx, &pipe) {
                    eprintln!("Worker error: {e}");
                    eprintln!("{}", e.backtrace());
                    process::exit(1);
                }
            })
            .context("Failed to spawn segment worker")?;

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
        let Ok(url) = url_rx.recv() else {
            return Ok(());
        };

        let mut pipe = pipe.lock().unwrap();

        if copy_segment(url, &mut pipe)?.is_some() {
            return Ok(());
        }

        sync_tx.send(()).context("Failed to sync from segment worker")?;
        loop {
            let Ok(url) = url_rx.recv() else {
                return Ok(());
            };

            if copy_segment(url, &mut pipe)?.is_some() {
                return Ok(());
            }
        }
    }
}

fn copy_segment(url: Url, pipe: &mut ChildStdin) -> Result<Option<()>> {
    if let Err(e) = CLIENT.get(url).send()?.copy_to(pipe) {
        match e.source() {
            Some(s) => match s.downcast_ref::<io::Error>() {
                Some(s) => match s.kind() {
                    BrokenPipe => return Ok(Some(())),
                    _ => return Err(e.into()),
                },
                _ => return Err(e.into()),
            },
            _ => return Err(e.into()),
        }
    }

    Ok(None)
}
