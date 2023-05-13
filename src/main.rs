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

use std::{
    process,
    process::{Child, ChildStdin, Command, ExitStatus, Stdio},
    thread,
    time::Instant,
};

use anyhow::{Context, Result};
use log::{debug, info, warn};
use pico_args::Arguments;
use simplelog::{
    format_description, ColorChoice, ConfigBuilder, LevelFilter, TermLogger, TerminalMode,
};

mod hls;
mod http;
mod segment_worker;
use hls::{MasterPlaylist, MediaPlaylist};
use segment_worker::Worker;

pub(crate) struct Player {
    process: Child,
}

impl Drop for Player {
    fn drop(&mut self) {
        if let Err(e) = self.process.kill() {
            warn!("Failed to kill player: {e}");
        }
    }
}

impl Player {
    pub fn spawn(path: &str, args: &str) -> Result<Self> {
        info!("Opening player: {} {}", path, args);
        Ok(Self {
            process: Command::new(path)
                .args(args.split_whitespace())
                .stdin(Stdio::piped())
                .spawn()
                .context("Failed to open player")?,
        })
    }

    pub fn stdin(&mut self) -> Result<ChildStdin> {
        self.process
            .stdin
            .take()
            .context("Failed to open player stdin")
    }

    pub fn wait(&mut self) -> Result<ExitStatus> {
        Ok(self.process.wait()?)
    }
}

#[derive(Default, Debug)]
struct Args {
    servers: Vec<String>,
    player_path: String,
    player_args: String,
    debug: bool,
    max_retries: u32,
    passthrough: bool,
    channel: String,
    quality: String,
}

impl Args {
    pub fn parse() -> Result<Self> {
        const DEFAULT_PLAYER_ARGS: &str = "-";
        const DEFAULT_MAX_RETRIES: u32 = 30;

        let mut parser = Arguments::from_env();
        if parser.contains("-h") || parser.contains("--help") {
            eprintln!(include_str!("usage"));
            process::exit(0);
        }

        if parser.contains("-V") || parser.contains("--version") {
            eprintln!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
            process::exit(0);
        }

        let debug = parser.contains("-d") || parser.contains("--debug");
        let channel = parser
            .free_from_str::<String>()?
            .to_lowercase()
            .replace("twitch.tv/", "");
        let quality = parser.free_from_str::<String>()?;
        let servers = parser
            .value_from_str::<&str, String>("-s")?
            .replace("[channel]", &channel)
            .split(',')
            .map(String::from)
            .collect();

        if parser.contains("--passthrough") {
            Ok(Self {
                passthrough: true,
                servers,
                debug,
                channel,
                quality,
                ..Default::default()
            })
        } else {
            Ok(Self {
                passthrough: false,
                servers,
                debug,
                channel,
                quality,
                player_path: parser.value_from_str("-p")?,
                player_args: parser
                    .opt_value_from_str("-a")?
                    .unwrap_or_else(|| DEFAULT_PLAYER_ARGS.to_owned()),
                max_retries: parser
                    .opt_value_from_str("--max-retries")?
                    .unwrap_or(DEFAULT_MAX_RETRIES),
            })
        }
    }
}

fn init_playlist(args: &Args, master_playlist: &MasterPlaylist) -> Result<MediaPlaylist> {
    let url = master_playlist.fetch(&args.channel, &args.quality)?;
    if args.passthrough {
        println!("{url}");
        process::exit(0);
    }

    match MediaPlaylist::new(&url) {
        Ok(playlist) => Ok(playlist),
        Err(e) => match e.downcast_ref::<hls::Error>() {
            Some(hls::Error::InvalidPrefetchUrl) => {
                info!("Stream is not low latency, opening player with playlist URL");
                let player_args = args
                    .player_args
                    .split_whitespace()
                    .map(|s| if s == "-" { url.clone() } else { s.to_owned() })
                    .collect::<Vec<String>>()
                    .join(" ");

                let mut player = Player::spawn(&args.player_path, &player_args)?;
                player.wait()?;

                process::exit(0);
            }
            _ => Err(e),
        },
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
        let mut playlist = match init_playlist(&args, &master_playlist) {
            Ok(playlist) => playlist,
            Err(e) => match e.downcast_ref::<hls::Error>() {
                Some(hls::Error::Advertisement | hls::Error::Discontinuity) => {
                    warn!("{e} on startup, resetting...");
                    continue;
                }
                _ => return Err(e),
            },
        };

        let worker = Worker::new(args.player_path.clone(), args.player_args.clone())?;
        worker.send(&playlist.prefetch_urls[0])?;
        worker.sync()?;

        let mut retry_count: u32 = 0;
        loop {
            let time = Instant::now();
            match playlist.reload() {
                Ok(_) => retry_count = 0,
                Err(e) => match e.downcast_ref::<hls::Error>() {
                    Some(hls::Error::Unchanged | hls::Error::InvalidPrefetchUrl) => {
                        retry_count += 1;
                        if retry_count == args.max_retries {
                            info!("Maximum retries on media playlist reached, exiting...");
                            return Ok(());
                        }

                        debug!("{e}, retrying...");
                        continue;
                    }
                    Some(hls::Error::Advertisement) => {
                        warn!("{e}, resetting...");
                        break;
                    }
                    Some(hls::Error::Discontinuity) => {
                        warn!("{e}, stream may be broken");
                    }
                    _ => return Err(e),
                },
            }

            worker.send(&playlist.prefetch_urls[1])?;

            if let Some(sleep_time) = playlist.duration.checked_sub(time.elapsed()) {
                thread::sleep(sleep_time);
            }
        }
    }
}
