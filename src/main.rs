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
use hls::{Error as HlsErr, MediaPlaylist, PrefetchUrlKind};
use player::Player;
use worker::{Error as WorkerErr, Worker};

static ARGS: OnceCell<Args> = OnceCell::new();

fn run(worker: &Worker, mut playlist: MediaPlaylist, max_retries: u32) -> Result<()> {
    worker.send(playlist.urls.take(PrefetchUrlKind::Newest)?)?;
    worker.sync()?;

    let mut retries: u32 = 0;
    loop {
        let time = Instant::now();
        match playlist.reload() {
            Ok(()) => retries = 0,
            Err(e) => match e.downcast_ref::<HlsErr>() {
                Some(HlsErr::Unchanged | HlsErr::InvalidPrefetchUrl | HlsErr::InvalidDuration) => {
                    retries += 1;
                    if retries == max_retries {
                        info!("Maximum retries on media playlist reached, exiting...");
                        return Ok(());
                    }

                    debug!("{e}, retrying...");
                    continue;
                }
                _ => return Err(e),
            },
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
        Err(e) => match e.downcast_ref::<HlsErr>() {
            Some(HlsErr::NotLowLatency(url)) => {
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
    match run(&worker, playlist, args.max_retries) {
        Ok(()) => Ok(()),
        Err(e) => match e.downcast_ref::<WorkerErr>() {
            Some(WorkerErr::SendFailed | WorkerErr::SyncFailed) => {
                info!("Player closed, exiting...");
                Ok(())
            }
            _ => Err(e),
        },
    }
}
