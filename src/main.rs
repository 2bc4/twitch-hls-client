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

fn main_loop(mut playlist: MediaPlaylist, mut worker: Worker) -> Result<()> {
    let mut prefetch_segment = PrefetchSegment::Newest;
    let mut prev_url = String::default();
    let mut unchanged_count = 0u32;
    loop {
        let time = Instant::now();

        playlist.reload()?;
        match playlist.prefetch_url(prefetch_segment) {
            Ok(url) if prev_url == url.as_str() => {
                if unchanged_count == 0 {
                    //already have the next segment, send it
                    info!("Playlist unchanged, fetching next segment...");
                    let url = playlist.prefetch_url(PrefetchSegment::Newest)?;
                    prev_url = url.as_str().to_owned();

                    worker.sync_url(url)?;
                } else {
                    info!("Playlist unchanged, retrying...");
                    playlist.duration()?.sleep_half(time.elapsed());
                }

                unchanged_count += 1;
                continue;
            }
            Ok(mut url) => {
                //at least a full segment duration has passed
                if unchanged_count > 2 {
                    prefetch_segment = PrefetchSegment::Newest; //catch up
                    url = playlist.prefetch_url(prefetch_segment)?;
                }
                unchanged_count = 0;
                prev_url = url.as_str().to_owned();

                match prefetch_segment {
                    PrefetchSegment::Newest => {
                        worker.sync_url(url)?;
                        prefetch_segment = PrefetchSegment::Next;
                    }
                    PrefetchSegment::Next => worker.url(url)?,
                };
            }
            Err(e) => match e.downcast_ref::<hls::Error>() {
                Some(hls::Error::Advertisement) => {
                    info!("Filtering ad segment...");
                    prefetch_segment = PrefetchSegment::Newest; //catch up when back
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
    let mut playlist = match MediaPlaylist::new(&args.hls, &agent) {
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

    let worker = Worker::spawn(
        Player::spawn(&args.player)?,
        playlist.header()?,
        agent.clone(),
    )?;
    drop(args);

    match main_loop(playlist, worker) {
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
