use std::{
    fmt::{self, Display, Formatter},
    sync::mpsc::{self, Receiver, Sender},
    thread::{self, JoinHandle},
};

use anyhow::{Context, Result};
use log::{debug, info};

use crate::{
    http::{Agent, Method, StatusError, Url},
    output::{Output, Writer},
};

#[derive(Debug)]
pub struct DeadError;

impl std::error::Error for DeadError {}

impl Display for DeadError {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "Worker died unexpectantly")
    }
}

pub struct Worker {
    handle: JoinHandle<Result<()>>,
    url_tx: Sender<Url>,
}

impl Worker {
    pub fn spawn(mut writer: Writer, header_url: Option<Url>, agent: Agent) -> Result<Self> {
        let (url_tx, url_rx): (Sender<Url>, Receiver<Url>) = mpsc::channel();

        let handle = thread::Builder::new()
            .name("worker".to_owned())
            .spawn(move || -> Result<()> {
                debug!("Starting");

                if let Some(header_url) = header_url {
                    let mut request = agent.binary(Vec::new());
                    request.call(Method::Get, &header_url)?;

                    writer.set_header(&request.into_writer())?;
                }

                let mut request = agent.binary(writer);
                loop {
                    let Ok(url) = url_rx.recv() else {
                        debug!("Exiting");
                        return Ok(());
                    };

                    match request.call(Method::Get, &url) {
                        Ok(()) => (),
                        Err(e) if StatusError::is_not_found(&e) => {
                            info!("Segment not found, skipping ahead...");
                            url_rx.try_iter().for_each(drop);
                        }
                        Err(e) => return Err(e),
                    }
                }
            })
            .context("Failed to spawn worker")?;

        Ok(Self { handle, url_tx })
    }

    pub fn url(&self, url: Url) -> Result<()> {
        if self.handle.is_finished() {
            return Err(DeadError.into());
        }

        self.url_tx.send(url)?;
        Ok(())
    }

    pub fn join(self) -> Result<()> {
        drop(self.url_tx);
        self.handle.join().expect("Worker panicked")
    }
}
