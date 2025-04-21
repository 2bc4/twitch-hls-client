use std::{
    fmt::{self, Display, Formatter},
    sync::mpsc::{self, Sender},
    thread::{Builder as ThreadBuilder, JoinHandle},
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
        f.write_str("Worker died unexpectantly")
    }
}

enum Reason {
    Wait(Writer),
    Killed,
}

pub struct Worker {
    handle: JoinHandle<Result<Reason>>,
    sender: Sender<Url>,
}

impl Worker {
    pub fn spawn(writer: Writer, agent: Agent) -> Result<Self> {
        let (sender, receiver) = mpsc::channel::<Url>();
        let handle = ThreadBuilder::new()
            .name("worker".to_owned())
            .spawn(move || -> Result<Reason> {
                debug!("Starting");

                let mut request = agent.binary(writer);
                loop {
                    let Ok(url) = receiver.recv() else {
                        debug!("Exiting");
                        return Ok(Reason::Killed);
                    };

                    match request.call(Method::Get, &url) {
                        Ok(()) => (),
                        Err(e) if StatusError::is_not_found(&e) => {
                            info!("Segment not found, skipping ahead...");
                            receiver.try_iter().for_each(drop);
                        }
                        Err(e) => return Err(e),
                    }

                    if request.get_ref().should_wait() {
                        return Ok(Reason::Wait(request.into_writer()));
                    }
                }
            })
            .context("Failed to spawn worker")?;

        Ok(Self { handle, sender })
    }

    pub fn url(&self, url: Url) -> Result<()> {
        if self.sender.send(url).is_err() {
            return Err(DeadError.into());
        }

        Ok(())
    }

    pub fn join(self) -> Result<Writer> {
        drop(self.sender);

        match self.handle.join().expect("Worker panicked") {
            Ok(Reason::Wait(writer)) => Ok(writer),
            Ok(Reason::Killed) => Err(DeadError.into()),
            Err(e) => Err(e),
        }
    }
}
