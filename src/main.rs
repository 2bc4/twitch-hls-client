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
#![deny(warnings)]
#![deny(clippy::pedantic)]

mod args;
mod hls;
mod http;
mod segment_worker;

use std::{thread, time::Instant};

use anyhow::Result;
use log::{debug, info, warn};
use simplelog::{
    format_description, ColorChoice, ConfigBuilder, LevelFilter, TermLogger, TerminalMode,
};

use args::Args;
use hls::{Error as HlsErr, MasterPlaylist, MediaPlaylist, PrefetchUrlKind};
use segment_worker::{Player, Worker};

enum Reason {
    Reset,
    Exit,
}

fn run(worker: &Worker, mut playlist: MediaPlaylist, max_retries: u32) -> Result<Reason> {
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
                        return Ok(Reason::Exit);
                    }

                    debug!("{e}, retrying...");
                    continue;
                }
                Some(HlsErr::Advertisement) => {
                    warn!("{e}, resetting...");
                    return Ok(Reason::Reset);
                }
                Some(HlsErr::Discontinuity) => {
                    warn!("{e}, stream may be broken");
                }
                _ => return Err(e),
            },
        }

        let segment_url = playlist.urls.take(PrefetchUrlKind::Next)?;
        if worker.send(segment_url).is_err() {
            info!("Player closed, exiting...");
            return Ok(Reason::Exit);
        }

        if let Some(sleep_time) = playlist.duration.checked_sub(time.elapsed()) {
            thread::sleep(sleep_time);
        }
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
            ConfigBuilder::new()
                .set_time_level(LevelFilter::Off)
                .build(),
            TerminalMode::Stderr,
            ColorChoice::Auto,
        )?;
    }
    debug!("{:?}", args);

    let master_playlist = MasterPlaylist::new(&args.servers)?;
    loop {
        let playlist_url = master_playlist.fetch_variant_playlist(&args.channel, &args.quality)?;
        if args.passthrough {
            println!("{playlist_url}");
            return Ok(());
        }

        let playlist = match MediaPlaylist::new(playlist_url) {
            Ok(playlist) => playlist,
            Err(e) => match e.downcast_ref::<HlsErr>() {
                Some(HlsErr::Advertisement | HlsErr::Discontinuity) => {
                    warn!("{e} on startup, resetting...");
                    continue;
                }
                Some(HlsErr::NotLowLatency(url)) => {
                    info!("{e}, opening player with playlist URL");
                    let player_args = args
                        .player_args
                        .split_whitespace()
                        .map(|s| if s == "-" { url.clone() } else { s.to_owned() })
                        .collect::<Vec<String>>()
                        .join(" ");

                    let mut player = Player::spawn(&args.player_path, &player_args)?;
                    player.wait()?;

                    return Ok(());
                }
                _ => return Err(e),
            },
        };

        let worker = Worker::new(Player::spawn(&args.player_path, &args.player_args)?)?;
        match run(&worker, playlist, args.max_retries) {
            Ok(reason) => match reason {
                Reason::Reset => continue,
                Reason::Exit => return Ok(()),
            },
            Err(e) => return Err(e),
        }
    }
}
