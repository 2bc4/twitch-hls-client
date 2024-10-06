use std::{
    sync::mpsc::{self, Receiver, Sender},
    thread::{self, JoinHandle},
};

use anyhow::{ensure, Context, Result};
use log::{debug, info};

use crate::{
    http::{Agent, Method, StatusError, Url},
    output::Writer,
};

pub struct Worker {
    //Option to call take() because handle.join() consumes self
    handle: Option<JoinHandle<Result<()>>>,
    url_tx: Sender<Url>,
}

impl Worker {
    pub fn spawn(writer: Writer, header_url: Option<Url>, agent: Agent) -> Result<Self> {
        let (url_tx, url_rx): (Sender<Url>, Receiver<Url>) = mpsc::channel();

        let handle = thread::Builder::new()
            .name("worker".to_owned())
            .spawn(move || -> Result<()> {
                debug!("Starting");

                let mut request = agent.binary(writer);
                if let Some(header_url) = header_url {
                    request.call(Method::Get, &header_url)?;
                }

                loop {
                    let Ok(url) = url_rx.recv() else {
                        debug!("Exiting");
                        return Ok(());
                    };

                    match request.call(Method::Get, &url) {
                        Ok(()) => (),
                        Err(e) if StatusError::is_not_found(&e) => {
                            info!("Segment not found, skipping ahead...");
                            for _ in url_rx.try_iter() {} //consume all
                        }
                        Err(e) => return Err(e),
                    }
                }
            })
            .context("Failed to spawn worker")?;

        Ok(Self {
            handle: Some(handle),
            url_tx,
        })
    }

    pub fn url(&mut self, url: Url) -> Result<()> {
        if self
            .handle
            .as_ref()
            .expect("Missing worker handle")
            .is_finished()
        {
            let result = self
                .handle
                .take()
                .expect("Missing worker handle while joining worker")
                .join()
                .expect("Worker panicked");

            ensure!(result.is_err(), "Worker died");
            return result;
        }

        self.url_tx.send(url)?;
        Ok(())
    }
}
