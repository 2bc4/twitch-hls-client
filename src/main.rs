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
use simplelog::{format_description, ColorChoice, ConfigBuilder, LevelFilter, TermLogger, TerminalMode};

use args::Args;
use hls::{Error as HlsErr, MediaPlaylist, PrefetchUrlKind};
use player::Player;
use worker::{Error as WorkerErr, Worker};

fn run(mut player: Player, mut playlist: MediaPlaylist, max_retries: u32) -> Result<()> {
    let mut worker = Worker::new(player.stdin())?;
    worker.send(playlist.urls.take(PrefetchUrlKind::Newest)?)?;
    worker.sync()?;

    let mut retry_count: u32 = 0;
    loop {
        let time = Instant::now();
        match playlist.reload() {
            Ok(()) => retry_count = 0,
            Err(e) => match e.downcast_ref::<HlsErr>() {
                Some(HlsErr::Unchanged | HlsErr::InvalidPrefetchUrl | HlsErr::InvalidDuration) => {
                    retry_count += 1;
                    if retry_count == max_retries {
                        info!("Maximum retries on media playlist reached, exiting...");
                        return Ok(());
                    }

                    debug!("{e}, retrying...");
                    continue;
                }
                _ => return Err(e),
            },
        }

        let next_url = playlist.urls.take(PrefetchUrlKind::Next)?;
        let newest_url = playlist.urls.take(PrefetchUrlKind::Newest)?;
        if next_url.host_str().unwrap() == newest_url.host_str().unwrap() {
            worker.send(next_url)?;
        } else {
            worker.send(next_url)?;

            worker = Worker::new(player.stdin())?;
            worker.send(newest_url)?;
            worker.sync()?;
        }

        playlist.sleep_segment_duration(time.elapsed());
    }
}

fn main() -> Result<()> {
    let args = Args::parse()?;
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

    let playlist_url = match args.servers {
        Some(servers) => hls::fetch_proxy_playlist(&servers, &args.channel, &args.quality),
        None => hls::fetch_twitch_playlist(&args.client_id, &args.auth_token, &args.channel, &args.quality),
    };

    let playlist_url = match playlist_url {
        Ok(playlist_url) => playlist_url,
        Err(e) => match e.downcast_ref::<HlsErr>() {
            Some(HlsErr::NotLowLatency(url)) => {
                info!("{e}, opening player with playlist URL");
                Player::spawn_and_wait(&args.player, &args.player_args, url)?;
                return Ok(());
            }
            _ => return Err(e),
        },
    };

    if args.passthrough {
        println!("{playlist_url}");
        return Ok(());
    }

    let playlist = MediaPlaylist::new(playlist_url)?;
    let player = Player::spawn(&args.player, &args.player_args)?;
    match run(player, playlist, args.max_retries) {
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
