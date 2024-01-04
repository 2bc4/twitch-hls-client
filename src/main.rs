#![forbid(unsafe_code)]
#![deny(warnings)]
#![deny(clippy::pedantic)]

mod args;
mod constants;
mod hls;
mod http;
mod player;
mod worker;

use std::time::Instant;

use anyhow::Result;
use log::{debug, info};
use once_cell::sync::OnceCell;
use simplelog::{format_description, ColorChoice, ConfigBuilder, LevelFilter, TermLogger, TerminalMode};

use args::Args;
use hls::{MediaPlaylist, PrefetchUrlKind};
use player::Player;
use worker::Worker;

static ARGS: OnceCell<Args> = OnceCell::new();

fn run(worker: &Worker, mut playlist: MediaPlaylist) -> Result<()> {
    worker.send(playlist.urls.take(PrefetchUrlKind::Newest)?)?;
    worker.sync()?;

    loop {
        let time = Instant::now();
        if let Err(e) = playlist.reload() {
            match e.downcast_ref::<hls::Error>() {
                Some(hls::Error::Unchanged) => {
                    debug!("{e}, retrying in half segment duration...");
                    playlist.sleep_half_segment_duration(time.elapsed());
                    continue;
                }
                _ => return Err(e),
            };
        }

        worker.send(playlist.urls.take(PrefetchUrlKind::Next)?)?;
        playlist.sleep_segment_duration(time.elapsed());
    }
}

fn main() -> Result<()> {
    let args = ARGS.get_or_try_init(Args::parse)?;
    if args.debug {
        TermLogger::init(
            LevelFilter::Debug,
            ConfigBuilder::new()
                .set_time_format_custom(format_description!(
                    "[hour]:[minute]:[second].[subsecond digits:5]"
                ))
                .set_time_offset_to_local()
                .unwrap() //isn't an error
                .build(),
            TerminalMode::Stderr,
            ColorChoice::Auto,
        )?;
    } else {
        TermLogger::init(
            LevelFilter::Info,
            ConfigBuilder::new().set_time_level(LevelFilter::Off).build(),
            TerminalMode::Stderr,
            ColorChoice::Auto,
        )?;
    }
    debug!("{:?}", args);

    let playlist_url = args.servers.as_ref().map_or_else(
        || hls::fetch_twitch_playlist(&args.client_id, &args.auth_token, &args.channel, &args.quality),
        |servers| hls::fetch_proxy_playlist(servers, &args.channel, &args.quality),
    );

    let playlist_url = match playlist_url {
        Ok(playlist_url) => playlist_url,
        Err(e) => match e.downcast_ref::<hls::Error>() {
            Some(hls::Error::NotLowLatency(url)) => {
                info!("{e}, opening player with playlist URL");
                Player::spawn_and_wait(&args.player, &args.player_args, url, args.quiet)?;
                return Ok(());
            }
            _ => return Err(e),
        },
    };

    if args.passthrough {
        println!("{playlist_url}");
        return Ok(());
    }

    let playlist = MediaPlaylist::new(&playlist_url)?;
    let mut player = Player::spawn(&args.player, &args.player_args, args.quiet)?;
    let worker = Worker::new(player.stdin()?)?;
    match run(&worker, playlist) {
        Ok(()) => Ok(()),
        Err(e) => {
            if http::Error::is_not_found(&e) {
                info!("Stream ended, exiting...");
                return Ok(());
            }

            if matches!(
                e.downcast_ref::<worker::Error>(),
                Some(worker::Error::SendFailed | worker::Error::SyncFailed)
            ) {
                info!("Player closed, exiting...");
                return Ok(());
            }

            Err(e)
        }
    }
}
