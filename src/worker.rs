use std::{
    sync::mpsc::{self, Receiver, Sender},
    thread::{self, JoinHandle},
};

use anyhow::{ensure, Context, Result};
use log::{debug, info};

use crate::{
    http::{Agent, StatusError, Url},
    output::OutputWriter,
};

pub struct Worker {
    //Option to call take() because handle.join() consumes self
    handle: Option<JoinHandle<Result<()>>>,
    url_tx: Sender<Url>,
}

impl Worker {
    pub fn spawn(writer: OutputWriter, header_url: Option<Url>, agent: Agent) -> Result<Self> {
        let (url_tx, url_rx): (Sender<Url>, Receiver<Url>) = mpsc::channel();

        let handle = thread::Builder::new()
            .name("worker".to_owned())
            .spawn(move || -> Result<()> {
                debug!("Starting");
                let mut request = {
                    let Ok(initial_url) = url_rx.recv() else {
                        debug!("Exiting before initial url");
                        return Ok(());
                    };

                    if let Some(header_url) = header_url {
                        let mut request = agent.request(writer, header_url)?;
                        request.call()?;
                        request.url(initial_url)?;

                        request
                    } else {
                        agent.request(writer, initial_url)?
                    }
                };

                request.call()?;
                loop {
                    let Ok(url) = url_rx.recv() else {
                        debug!("Exiting");
                        return Ok(());
                    };

                    request.url(url)?;
                    match request.call() {
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
        self.join_if_dead()?;
        self.url_tx.send(url)?;

        Ok(())
    }

    fn join_if_dead(&mut self) -> Result<()> {
        if self
            .handle
            .as_ref()
            .context("Worker handle None")?
            .is_finished()
        {
            let result = self
                .handle
                .take()
                .context("Handle None while joining worker")?
                .join()
                .expect("Worker panicked");

            ensure!(result.is_err(), "Worker died");
            return result;
        }

        Ok(())
    }
}
