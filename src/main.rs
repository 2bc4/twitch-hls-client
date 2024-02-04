mod args;
mod constants;
mod hls;
mod http;
mod logger;
mod player;
mod worker;

use std::{
    io::{self, ErrorKind::BrokenPipe},
    time::Instant,
};

use anyhow::Result;
use log::{debug, info};

use args::Args;
use hls::{LowLatencyHandler, MasterPlaylist, MediaPlaylist, NormalLatencyHandler, SegmentHandler};
use http::Agent;
use logger::Logger;
use player::Player;
use worker::Worker;

fn main_loop(mut handler: impl SegmentHandler) -> Result<()> {
    handler.process(Instant::now())?;
    loop {
        let time = Instant::now();

        handler.reload()?;
        handler.process(time)?;
    }
}

fn main() -> Result<()> {
    let (playlist, worker, low_latency) = {
        let mut args = Args::new()?;

        Logger::init(args.debug)?;
        debug!("{args:?}");

        let agent = Agent::new(&args.http)?;
        let master_playlist = match MasterPlaylist::new(&args.hls, &agent) {
            Ok(playlist) if args.passthrough => {
                return Player::passthrough(&mut args.player, &playlist.url)
            }
            Ok(playlist) => playlist,
            Err(e) => match e.downcast_ref::<hls::Error>() {
                Some(hls::Error::Offline) => {
                    info!("{e}, exiting...");
                    return Ok(());
                }
                _ => return Err(e),
            },
        };

        let playlist = MediaPlaylist::new(&master_playlist, &agent)?;
        let worker = Worker::spawn(
            Player::spawn(&args.player)?,
            playlist.header()?,
            agent.clone(),
        )?;

        (playlist, worker, master_playlist.low_latency)
    };

    let result = if low_latency {
        main_loop(LowLatencyHandler::new(playlist, worker))
    } else {
        main_loop(NormalLatencyHandler::new(playlist, worker))
    };

    match result {
        Ok(()) => Ok(()),
        Err(e) => {
            if matches!(e.downcast_ref::<hls::Error>(), Some(hls::Error::Offline)) {
                info!("Stream ended, exiting...");
                return Ok(());
            }

            if let Some(e) = e.downcast_ref::<io::Error>() {
                if matches!(e.kind(), BrokenPipe) {
                    info!("Player closed, exiting...");
                    return Ok(());
                }
            }

            Err(e)
        }
    }
}
