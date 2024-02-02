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
use hls::{MediaPlaylist, PrefetchSegment};
use http::Agent;
use logger::Logger;
use player::Player;
use worker::Worker;

fn main_loop(mut playlist: MediaPlaylist, player: Player, agent: &Agent) -> Result<()> {
    let mut worker = Worker::spawn(player, playlist.header()?, agent.clone())?;

    let mut prefetch_segment = PrefetchSegment::Newest;
    let mut prev_url = String::default();
    loop {
        let time = Instant::now();

        playlist.reload()?;
        match playlist.prefetch_url(prefetch_segment) {
            Ok(url) if prev_url == url.as_str() => {
                info!("Playlist unchanged, retrying...");
                playlist.duration()?.sleep_half(time.elapsed());
                continue;
            }
            Ok(url) => {
                prev_url = url.as_str().to_owned();

                if prefetch_segment == PrefetchSegment::Newest {
                    worker.sync_url(url)?;
                    prefetch_segment = PrefetchSegment::Next;
                } else {
                    worker.url(url)?;
                }
            }
            Err(e) => match e.downcast_ref::<hls::Error>() {
                Some(hls::Error::Advertisement) => {
                    info!("Filtering ad segment...");
                    prefetch_segment = PrefetchSegment::Newest; //catch up
                }
                _ => return Err(e),
            },
        };

        playlist.duration()?.sleep(time.elapsed());
    }
}

fn main() -> Result<()> {
    let mut args = Args::new()?;

    Logger::init(args.debug)?;
    debug!("{args:?}");

    let agent = Agent::new(&args.http)?;
    let playlist = match MediaPlaylist::new(&args.hls, &agent) {
        Ok(mut playlist) if args.passthrough => {
            return Player::passthrough(&mut args.player, &playlist.url()?)
        }
        Ok(playlist) => playlist,
        Err(e) => match e.downcast_ref::<hls::Error>() {
            Some(hls::Error::Offline) => {
                info!("{e}, exiting...");
                return Ok(());
            }
            Some(hls::Error::NotLowLatency(url)) => {
                info!("{e}");
                return Player::passthrough(&mut args.player, url);
            }
            _ => return Err(e),
        },
    };

    let player = Player::spawn(&args.player)?;
    drop(args);

    match main_loop(playlist, player, &agent) {
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
