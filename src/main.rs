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
    loop {
        let time = Instant::now();

        playlist.reload()?;
        match playlist.next() {
            Ok(next) => worker.url(next)?,
            Err(e) => {
                if matches!(e.downcast_ref::<hls::Error>(), Some(hls::Error::Unchanged)) {
                    debug!("{e}, retrying in half segment duration...");
                    playlist.duration()?.sleep_half(time.elapsed());
                    continue;
                }

                return Err(e);
            }
        };

        playlist.duration()?.sleep(time.elapsed());
    }
}

fn main() -> Result<()> {
    let (args, http_args) = Args::parse()?;

    Logger::init(args.debug)?;
    debug!("{:?} {:?}", args, http_args);

    let agent = Agent::new(http_args);
    let playlist_url = match args.servers.as_ref().map_or_else(
        || hls::fetch_twitch_playlist(&args.client_id, &args.auth_token, &args.hls, &agent),
        |servers| hls::fetch_proxy_playlist(servers, &args.hls, &agent),
    ) {
        Ok(playlist_url) => playlist_url,
        Err(e) => match e.downcast_ref::<hls::Error>() {
            Some(hls::Error::Offline) => {
                info!("{e}, exiting...");
                return Ok(());
            }
            Some(hls::Error::NotLowLatency(playlist_url)) => {
                info!("{e}");
                return Player::passthrough(&args.player, playlist_url);
            }
            _ => return Err(e),
        },
    };

    if args.passthrough {
        return Player::passthrough(&args.player, &playlist_url);
    }

    let playlist = MediaPlaylist::new(&playlist_url, &agent)?;
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
