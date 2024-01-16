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
use once_cell::sync::OnceCell;

use args::Args;
use hls::{MediaPlaylist, PrefetchUrlKind, SleepLength};
use logger::Logger;
use player::Player;
use worker::Worker;

static ARGS: OnceCell<Args> = OnceCell::new();

fn main_loop(mut playlist: MediaPlaylist, player: Player) -> Result<()> {
    let mut worker = Worker::spawn(
        player,
        playlist.urls.take(PrefetchUrlKind::Newest)?,
        playlist.header_url.0.take(),
    )?;

    loop {
        let time = Instant::now();
        if let Err(e) = playlist.reload() {
            if matches!(e.downcast_ref::<hls::Error>(), Some(hls::Error::Unchanged)) {
                debug!("{e}, retrying in half segment duration...");
                playlist.duration.sleep(SleepLength::Half, time.elapsed());
                continue;
            }

            return Err(e);
        }

        worker.url(playlist.urls.take(PrefetchUrlKind::Next)?)?;
        playlist.duration.sleep(SleepLength::Full, time.elapsed());
    }
}

fn main() -> Result<()> {
    let args = ARGS.get_or_try_init(Args::parse)?;

    Logger::init(args.debug)?;
    debug!("{:?}", args);

    let playlist_url = match args.servers.as_ref().map_or_else(
        || {
            hls::fetch_twitch_playlist(
                &args.client_id,
                &args.auth_token,
                &args.codecs,
                &args.channel,
                &args.quality,
            )
        },
        |servers| hls::fetch_proxy_playlist(servers, &args.codecs, &args.channel, &args.quality),
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

    let playlist = MediaPlaylist::new(&playlist_url)?;
    let player = Player::spawn(&args.player)?;
    match main_loop(playlist, player) {
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
