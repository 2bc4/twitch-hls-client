//    Copyright (C) 2023 2bc4
//
//    This program is free software: you can redistribute it and/or modify
//    it under the terms of the GNU General Public License as published by
//    the Free Software Foundation, either version 3 of the License, or
//    (at your option) any later version.
//
//    This program is distributed in the hope that it will be useful,
//    but WITHOUT ANY WARRANTY; without even the implied warranty of
//    MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
//    GNU General Public License for more details.
//
//    You should have received a copy of the GNU General Public License
//    along with this program.  If not, see <https://www.gnu.org/licenses/>.

#![forbid(unsafe_code)]

use std::{io, process, thread, time::Instant};

use anyhow::Result;
use clap::Parser;
use log::{debug, info, warn};
use simplelog::{ColorChoice, ConfigBuilder, LevelFilter, TermLogger, TerminalMode};

mod hls;
mod http;
mod segment_worker;
use hls::MediaPlaylist;
use http::Request;
use segment_worker::Worker;

#[derive(Parser)]
#[command(version, next_line_help = true)]
struct Args {
    /// Playlist proxy server to fetch the playlist from.
    /// Can be multiple comma separated servers, will try each in order until successful.
    /// If URL path is "[ttvlol]" the playlist will be requested using the TTVLOL API.
    /// If URL includes "[channel]" it will be replaced with the channel argument at runtime.
    #[arg(short, long, value_name = "URL", verbatim_doc_comment)]
    server: String,

    /// Path to the player that the stream will be piped to,
    /// if not specified will write stream to stdout
    #[arg(short, long = "player", value_name = "PATH")]
    player_path: Option<String>,

    /// Arguments to pass to the player
    #[arg(
        short = 'a',
        long,
        value_name = "ARGUMENTS",
        default_value = "",
        hide_default_value = true,
        allow_hyphen_values = true
    )]
    player_args: String,

    /// Enable debug logging
    #[arg(short, long)]
    debug: bool,

    /// Twitch channel to watch (can also be twitch.tv/channel for Streamlink compatibility)
    channel: String,

    /// Stream quality/variant playlist to fetch (best, 1080p, 720p, 360p, 160p, audio_only)
    quality: String,
}

//Only exists to check the errors of both reloads at once
fn kickstart(playlist: &mut MediaPlaylist, worker: &mut Worker) -> Result<Request> {
    playlist.reload()?;

    let mut request = Request::get(&playlist.prefetch_urls[0])?;
    io::copy(&mut request.reader()?, worker)?;

    playlist.reload()?;
    Ok(request)
}

fn reload_loop(playlist: &mut MediaPlaylist, worker: &mut Worker) -> Result<()> {
    let mut request = match kickstart(playlist, worker) {
        Ok(request) => request,
        Err(e) => match e.downcast_ref::<hls::Error>() {
            Some(hls::Error::Advertisement | hls::Error::Discontinuity) => {
                warn!("{e} on startup, resetting...");
                return Ok(());
            }
            _ => return Err(e),
        },
    };

    let mut retry_count = 0;
    loop {
        let time = Instant::now();
        let preconnect = request
            .is_different_host(&playlist.prefetch_urls[0])?
            .then(|| {
                debug!("Server changed, preconnecting to next");
                Request::get(&playlist.prefetch_urls[0])
            });

        request.set_url(&playlist.prefetch_urls[1])?;
        io::copy(&mut request.reader()?, worker)?;

        loop {
            match playlist.reload() {
                Ok(_) => {
                    retry_count = 0;
                    break;
                }
                Err(e) => match e.downcast_ref::<hls::Error>() {
                    Some(hls::Error::Unchanged | hls::Error::InvalidPrefetchUrl) => {
                        retry_count += 1;
                        if retry_count == 5 {
                            warn!("Maximum retries on media playlist reached, exiting...");
                            process::exit(0);
                        }

                        debug!("{e}, retrying...");
                        continue;
                    }
                    Some(hls::Error::Advertisement) => {
                        warn!("{e}, resetting...");
                        return Ok(());
                    }
                    Some(hls::Error::Discontinuity) => {
                        warn!("{e}, stream may be broken");
                    }
                    _ => return Err(e),
                },
            }
        }

        if let Some(preconnect) = preconnect {
            request = preconnect?;
        }

        let elapsed = time.elapsed();
        if elapsed < playlist.duration {
            let sleep_time = playlist.duration - elapsed;
            debug!(
                "Duration: {:?} | Elapsed: {:?} | Sleeping for {:?}",
                playlist.duration, elapsed, sleep_time
            );

            thread::sleep(sleep_time);
        } else {
            debug!("Duration: {:?} | Elapsed: {:?}", playlist.duration, elapsed);
        }
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
    if args.debug {
        TermLogger::init(
            LevelFilter::Debug,
            ConfigBuilder::new()
                .set_thread_level(LevelFilter::Off)
                .build(),
            TerminalMode::Stderr,
            ColorChoice::Auto,
        )?;
    } else {
        TermLogger::init(
            LevelFilter::Info,
            ConfigBuilder::new()
                .set_time_level(LevelFilter::Off)
                .build(),
            TerminalMode::Stderr,
            ColorChoice::Never,
        )?;
    }

    loop {
        let mut worker = Worker::new(&args.player_path, &args.player_args)?;
        worker.wait_until_ready()?;

        let mut playlist = MediaPlaylist::new(&args.server, &args.channel, &args.quality)?;

        if let Err(e) = reload_loop(&mut playlist, &mut worker) {
            match e.downcast_ref::<http::Error>() {
                Some(http::Error::Status(code, _)) if *code == 404 => {
                    info!("Playlist not found. Stream likely ended, exiting...");
                    return Ok(());
                }
                _ => return Err(e),
            }
        }
    }
}
