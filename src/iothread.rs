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
    process::ChildStdin,
    sync::mpsc::{channel, Receiver, Sender},
    thread,
};

use anyhow::{bail, Context, Result};
use log::{error, info};
use ureq::Agent;

pub struct IOThread {
    url_sender: Sender<String>,
}

impl IOThread {
    pub fn new(agent: &Agent, stdin: Option<ChildStdin>) -> Result<Self> {
        let (url_sender, url_receiver): (Sender<String>, Receiver<String>) = channel();

        let agent = agent.clone(); //ureq uses an ARC to store state
        thread::Builder::new()
            .name("IO Thread".to_owned())
            .spawn(move || {
                if let Err(e) = Self::thread_main(&agent, stdin, &url_receiver) {
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
        agent: &Agent,
        stdin: Option<ChildStdin>,
        url_receiver: &Receiver<String>,
    ) -> Result<()> {
        let mut pipe: Box<dyn Write> = stdin.map_or_else(
            || {
                info!("Writing to stdout");
                Box::new(io::stdout().lock()) as Box<dyn Write>
            },
            |stdin| Box::new(stdin) as Box<dyn Write>,
        );

        loop {
            let Ok(url) = url_receiver.recv() else { return Ok(()); };

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
                    process::exit(0);
                }
                Err(e) => bail!(e),
                _ => (),
            };
        }
    }
}
