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

use args::Args;
use hls::{
    playlist::{MasterPlaylist, MediaPlaylist},
    segment::Handler,
    OfflineError,
};
use http::Agent;
use logger::Logger;
use output::{OutputWriter, Player};
use worker::Worker;

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
        let mut args = Args::new()?;

        Logger::init(args.debug)?;
        debug!("{args:?}");

        let agent = Agent::new(&args.http)?;
        let mut master_playlist = match MasterPlaylist::new(&mut args.hls, &agent) {
            Ok(playlist) => playlist,
            Err(e) if e.downcast_ref::<OfflineError>().is_some() => {
                info!("{e}, exiting...");
                return Ok(());
            }
            Err(e) => return Err(e),
        };

        let Some(variant_playlist) = args.quality.and_then(|q| master_playlist.find(&q)) else {
            info!("Available stream qualities: {master_playlist}");
            return Ok(());
        };

        if args.passthrough {
            return Player::passthrough(&mut args.output.player, &variant_playlist.url);
        }

        let mut playlist = MediaPlaylist::new(variant_playlist.url, &agent)?;
        let worker = Worker::spawn(
            OutputWriter::new(&args.output)?,
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
