mod args;
mod constants;
mod hls;
mod http;
mod logger;
mod output;
mod worker;

use std::{
    io::{self, ErrorKind::Other},
    time::Instant,
};

use anyhow::Result;
use log::{debug, info};

use args::{ArgParser, Parser};
use hls::{segment::Handler, MasterPlaylist, MediaPlaylist, OfflineError};
use http::Agent;
use logger::Logger;
use output::{OutputWriter, Player};
use worker::Worker;

#[derive(Default, Debug)]
pub struct Args {
    debug: bool,
    passthrough: bool,
}

impl ArgParser for Args {
    fn parse(&mut self, parser: &mut Parser) -> Result<()> {
        parser.parse_switch_or(&mut self.debug, "-d", "--debug")?;
        parser.parse_switch(&mut self.passthrough, "--passthrough")?;

        Ok(())
    }
}

fn main_loop(mut handler: Handler) -> Result<()> {
    handler.process(Instant::now())?;
    loop {
        let time = Instant::now();

        handler.playlist.reload()?;
        handler.process(time)?;
    }
}

fn main() -> Result<()> {
    let handler = {
        let (main_args, http_args, hls_args, mut output_args) = args::parse()?;

        Logger::init(main_args.debug)?;
        debug!("{main_args:?} {http_args:?} {hls_args:?} {output_args:?}");

        let agent = Agent::new(http_args)?;
        let mut master_playlist = match MasterPlaylist::new(hls_args, &agent) {
            Ok(playlist) => playlist,
            Err(e) if e.downcast_ref::<OfflineError>().is_some() => {
                info!("{e}, exiting...");
                return Ok(());
            }
            Err(e) => return Err(e),
        };

        let Some(url) = master_playlist.get_stream() else {
            info!("Available streams: {master_playlist}");
            return Ok(());
        };

        if main_args.passthrough {
            return Player::passthrough(&mut output_args.player, &url);
        }

        let mut playlist = MediaPlaylist::new(url, &agent)?;
        let worker = Worker::spawn(
            OutputWriter::new(&output_args)?,
            playlist.header.take(),
            agent.clone(),
        )?;

        Handler::new(playlist, worker)
    };

    match main_loop(handler) {
        Ok(()) => Ok(()),
        Err(e) => {
            if e.downcast_ref::<OfflineError>().is_some() {
                info!("Stream ended, exiting...");
                return Ok(());
            }

            //Currently the only Other error is thrown when player closed
            //so no need to check further.
            if e.downcast_ref::<io::Error>()
                .is_some_and(|e| e.kind() == Other)
            {
                info!("Player closed, exiting...");
                return Ok(());
            }

            Err(e)
        }
    }
}
