use std::{
    sync::mpsc::{self, Receiver, Sender, SyncSender},
    thread::{self, JoinHandle},
};

use anyhow::{ensure, Context, Result};
use log::debug;
use url::Url;

use crate::{http::Agent, player::Player};

struct ChannelData {
    url: Url,
    should_sync: bool,
}

pub struct Worker {
    //Option to call take() because handle.join() consumes self.
    //Will always be Some unless this throws an error.
    handle: Option<JoinHandle<Result<()>>>,
    url_tx: Sender<ChannelData>,
    sync_rx: Receiver<()>,
}

impl Worker {
    pub fn spawn(player: Player, header_url: Option<Url>, agent: Agent) -> Result<Self> {
        let (url_tx, url_rx): (Sender<ChannelData>, Receiver<ChannelData>) = mpsc::channel();
        let (sync_tx, sync_rx): (SyncSender<()>, Receiver<()>) = mpsc::sync_channel(1);

        let handle = thread::Builder::new()
            .name("worker".to_owned())
            .spawn(move || -> Result<()> {
                debug!("Starting");
                let mut request = {
                    let Ok(initial_data) = url_rx.recv() else {
                        debug!("Exiting before initial url");
                        return Ok(());
                    };
                    assert!(initial_data.should_sync);

                    if let Some(header_url) = header_url {
                        let mut request = agent.writer(player, &header_url)?;
                        request.call(&initial_data.url)?;

                        request
                    } else {
                        agent.writer(player, &initial_data.url)?
                    }
                };

                sync_tx.send(())?;
                loop {
                    let Ok(data) = url_rx.recv() else {
                        debug!("Exiting");
                        return Ok(());
                    };

                    request.call(&data.url)?;
                    if data.should_sync {
                        sync_tx.send(())?;
                    }
                }
            })
            .context("Failed to spawn worker")?;

        Ok(Self {
            handle: Some(handle),
            url_tx,
            sync_rx,
        })
    }

    pub fn sync_url(&mut self, url: Url) -> Result<()> {
        self.send(url, true)
    }

    pub fn url(&mut self, url: Url) -> Result<()> {
        self.send(url, false)
    }

    fn send(&mut self, url: Url, should_sync: bool) -> Result<()> {
        self.join_if_dead()?;

        debug!("Sending URL to worker: {url}");
        self.url_tx.send(ChannelData { url, should_sync })?;

        if should_sync {
            self.sync_rx.recv().or_else(|_| self.join_if_dead())?;
        }

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
