//    Copyright (C) 2023 2bc4
//
//    This program is free software: you can redistribute it and/or modify
//    it under the terms of the GNU General Public License as published by
//    the Free Software Foundation, either version 3 of the License, or
//    (at your option) any later version.
//
//    This program is distributed in the hope that it will be useful,
//    but WITHOUT ANY WARRANTY; without even the implied warranty of
//    MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
//    GNU General Public License for more details.
//
//    You should have received a copy of the GNU General Public License
//    along with this program.  If not, see <https://www.gnu.org/licenses/>.

use std::{
    io,
    io::{ErrorKind::BrokenPipe, Write},
    process,
    sync::mpsc::{channel, Receiver, Sender},
    thread,
};

use anyhow::{bail, Context, Result};
use log::{error, info};
use ureq::AgentBuilder;

use crate::USER_AGENT;

enum ExitReason {
    Killed,
    PipeClosed,
}

pub struct IOThread {
    url_sender: Sender<String>,
}

impl IOThread {
    pub fn new(pipe: Box<dyn Write + Send>) -> Result<Self> {
        let (url_sender, url_receiver): (Sender<String>, Receiver<String>) = channel();

        thread::Builder::new()
            .name("IO Thread".to_owned())
            .spawn(move || match Self::thread_main(pipe, &url_receiver) {
                Ok(reason) => match reason {
                    ExitReason::Killed => (),
                    ExitReason::PipeClosed => process::exit(0),
                },
                Err(e) => {
                    error!("Error: {}", e);
                    process::exit(1);
                }
            })
            .context("Error spawning IO thread")?;

        Ok(Self { url_sender })
    }

    pub fn send_url(&self, url: &str) -> Result<()> {
        self.url_sender
            .send(url.to_owned())
            .context("Error sending URL to IO thread")?;

        Ok(())
    }

    fn thread_main(
        mut pipe: Box<dyn Write + Send>,
        url_receiver: &Receiver<String>,
    ) -> Result<ExitReason> {
        let agent = AgentBuilder::new().user_agent(USER_AGENT).build();

        loop {
            let Ok(url) = url_receiver.recv() else { return Ok(ExitReason::Killed) };

            let mut reader = agent
                .get(&url)
                .set("content-type", "application/octet-stream")
                .set("referer", "https://player.twitch.tv")
                .set("origin", "https://player.twitch.tv")
                .set("connection", "keep-alive")
                .call()
                .context("Error fetching segment")?
                .into_reader();

            match io::copy(&mut reader, &mut pipe) {
                Err(ref e) if e.kind() == BrokenPipe => {
                    info!("Pipe closed, exiting...");
                    return Ok(ExitReason::PipeClosed);
                }
                Err(e) => bail!(e),
                _ => (),
            };
        }
    }
}
