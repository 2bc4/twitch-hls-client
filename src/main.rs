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
use hls::MediaPlaylist;
use http::Agent;
use logger::Logger;
use player::Player;
use worker::Worker;

fn main_loop(mut playlist: MediaPlaylist, player: Player, agent: &Agent) -> Result<()> {
    let mut worker = Worker::spawn(player, playlist.newest()?, playlist.header()?, agent)?;

    let mut prev_url = String::default();
    loop {
        let time = Instant::now();

        playlist.reload()?;
        match playlist.next() {
            Ok(url) if url.as_str() == prev_url => {
                info!("Playlist unchanged, retrying...");
                playlist.duration()?.sleep_half(time.elapsed());
                continue;
            }
            Ok(url) => {
                prev_url = url.as_str().to_owned();
                worker.url(url)?;
            }
            Err(_) => info!("Filtering ad segment..."),
        };

        playlist.duration()?.sleep(time.elapsed());
    }
}

fn main() -> Result<()> {
    let (args, http_args) = Args::parse()?;

    Logger::init(args.debug)?;
    debug!("{:?} {:?}", args, http_args);

    let agent = Agent::new(http_args);
    let playlist = match args.servers.as_ref().map_or_else(
        || hls::fetch_twitch_playlist(&args.client_id, &args.auth_token, &args.hls, &agent),
        |servers| hls::fetch_proxy_playlist(servers, &args.hls, &agent),
    ) {
        Ok(url) if args.passthrough => return Player::passthrough(&args.player, &url),
        Ok(url) => MediaPlaylist::new(&url, &agent)?,
        Err(e) => match e.downcast_ref::<hls::Error>() {
            Some(hls::Error::Offline) => {
                info!("{e}, exiting...");
                return Ok(());
            }
            Some(hls::Error::NotLowLatency(url)) => {
                info!("{e}");
                return Player::passthrough(&args.player, url);
            }
            _ => return Err(e),
        },
    };

    let player = Player::spawn(&args.player)?;
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
