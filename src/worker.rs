use std::{
    sync::{
        mpsc::{self, Receiver, Sender, SyncSender},
        Arc,
    },
    thread::{self, JoinHandle},
};

use anyhow::{ensure, Context, Result};
use log::debug;

use crate::{
    http::{Agent, Url},
    player::Player,
};

struct ChannelMessage {
    url: Arc<Url>,
    should_sync: bool,
}

pub struct Worker {
    //Option to call take() because handle.join() consumes self.
    //Will always be Some unless this throws an error.
    handle: Option<JoinHandle<Result<()>>>,
    url_tx: Sender<ChannelMessage>,
    sync_rx: Receiver<()>,
}

impl Worker {
    pub fn spawn(player: Player, header_url: Option<Url>, agent: Agent) -> Result<Self> {
        let (url_tx, url_rx): (Sender<ChannelMessage>, Receiver<ChannelMessage>) = mpsc::channel();
        let (sync_tx, sync_rx): (SyncSender<()>, Receiver<()>) = mpsc::sync_channel(1);

        let handle = thread::Builder::new()
            .name("worker".to_owned())
            .spawn(move || -> Result<()> {
                debug!("Starting");
                let mut request = {
                    let Ok(initial_msg) = url_rx.recv() else {
                        debug!("Exiting before initial url");
                        return Ok(());
                    };

                    let request = if let Some(header_url) = header_url {
                        let mut request = agent.writer(player, &header_url)?;
                        request.call(&initial_msg.url)?;

                        request
                    } else {
                        agent.writer(player, &initial_msg.url)?
                    };

                    if initial_msg.should_sync {
                        sync_tx.send(())?;
                    }

                    request
                };

                loop {
                    let Ok(msg) = url_rx.recv() else {
                        debug!("Exiting");
                        return Ok(());
                    };

                    request.call(&msg.url)?;
                    if msg.should_sync {
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

    pub fn sync_url(&mut self, url: Arc<Url>) -> Result<()> {
        self.send(url, false)
    }

    pub fn url(&mut self, url: Arc<Url>) -> Result<()> {
        self.send(url, false)
    }

    fn send(&mut self, url: Arc<Url>, should_sync: bool) -> Result<()> {
        self.join_if_dead()?;

        self.url_tx.send(ChannelMessage { url, should_sync })?;
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
