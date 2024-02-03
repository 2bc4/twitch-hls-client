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

struct UrlHandler {
    playlist: MediaPlaylist,
    worker: Worker,
    prefetch_kind: PrefetchSegment,
    prev_url: String,
    unchanged_count: u32,
}

impl UrlHandler {
    fn new(playlist: MediaPlaylist, worker: Worker) -> Self {
        Self {
            playlist,
            worker,
            prefetch_kind: PrefetchSegment::Newest,
            prev_url: String::default(),
            unchanged_count: u32::default(),
        }
    }

    fn process(&mut self, time: Instant) -> Result<()> {
        match self.playlist.prefetch_url(self.prefetch_kind) {
            Ok(url) if self.prev_url == url.as_str() => {
                if self.unchanged_count == 0 {
                    //already have the next segment, send it
                    info!("Playlist unchanged, fetching next segment...");
                    let url = self.playlist.prefetch_url(PrefetchSegment::Newest)?;
                    self.prev_url = url.as_str().to_owned();

                    self.worker.sync_url(url)?;
                } else {
                    info!("Playlist unchanged, retrying...");
                    self.playlist.duration()?.sleep_half(time.elapsed());
                }

                self.unchanged_count += 1;
                return Ok(());
            }
            Ok(mut url) => {
                //at least a full segment duration has passed
                if self.unchanged_count > 2 {
                    self.prefetch_kind = PrefetchSegment::Newest; //catch up
                    url = self.playlist.prefetch_url(self.prefetch_kind)?;
                }
                self.unchanged_count = 0;
                self.prev_url = url.as_str().to_owned();

                match self.prefetch_kind {
                    PrefetchSegment::Newest => {
                        self.worker.sync_url(url)?;
                        self.prefetch_kind = PrefetchSegment::Next;
                        return Ok(());
                    }
                    PrefetchSegment::Next => self.worker.url(url)?,
                };
            }
            Err(e) => match e.downcast_ref::<hls::Error>() {
                Some(hls::Error::Advertisement) => {
                    info!("Filtering ad segment...");
                    self.prefetch_kind = PrefetchSegment::Newest; //catch up when back
                }
                _ => return Err(e),
            },
        };

        self.playlist.duration()?.sleep(time.elapsed());
        Ok(())
    }
}

fn main_loop(mut handler: UrlHandler) -> Result<()> {
    handler.process(Instant::now())?;
    loop {
        let time = Instant::now();
        debug!("----------RELOADING----------");

        handler.playlist.reload()?;
        handler.process(time)?;
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

    match main_loop(UrlHandler::new(playlist, worker)) {
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
