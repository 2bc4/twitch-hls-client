use std::{
    sync::mpsc::{self, Receiver, Sender, SyncSender},
    thread::{self, JoinHandle},
};

use anyhow::{ensure, Context, Result};
use log::debug;
use url::Url;

use crate::{http::WriterRequest, player::Player};

pub struct Worker {
    //Option to call take() because handle.join() consumes self.
    //Will always be Some unless this throws an error.
    handle: Option<JoinHandle<Result<()>>>,
    url_tx: Sender<Url>,
}

impl Worker {
    pub fn spawn(player: Player, initial_url: Url) -> Result<Self> {
        let (url_tx, url_rx): (Sender<Url>, Receiver<Url>) = mpsc::channel();
        let (init_tx, init_rx): (SyncSender<()>, Receiver<()>) = mpsc::sync_channel(1);

        let handle = thread::Builder::new()
            .name("worker".to_owned())
            .spawn(move || -> Result<()> {
                debug!("Starting with URL: {initial_url}");
                let mut request = WriterRequest::get(player, &initial_url)?;

                init_tx.send(())?;
                loop {
                    let Ok(url) = url_rx.recv() else {
                        debug!("Exiting");
                        return Ok(());
                    };

                    request.call(&url)?;
                }
            })
            .context("Failed to spawn segment worker")?;

        let mut worker = Self {
            handle: Some(handle),
            url_tx,
        };

        init_rx.recv().or_else(|_| worker.join_if_dead())?;
        Ok(worker)
    }

    pub fn url(&mut self, url: Url) -> Result<()> {
        self.join_if_dead()?;

        debug!("Sending URL to worker: {url}");
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
