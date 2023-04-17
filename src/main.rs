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
    cmp::{Ord, Ordering},
    io,
    io::Write,
    process::{Command, Stdio},
    thread,
    time::Instant,
};

use anyhow::{bail, ensure, Context, Result};
use clap::Parser;
use is_terminal::IsTerminal;
use log::{debug, info, warn};
use simplelog::{ColorChoice, ConfigBuilder, LevelFilter, TermLogger, TerminalMode};
use ureq::AgentBuilder;

mod iothread;
mod playlist;
use iothread::IOThread;
use playlist::MediaPlaylist;

const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/111.0.0.0 Safari/537.36";

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

    /// Disables resetting the player and stream when encountering an embedded advertisement
    #[arg(long)]
    disable_reset_on_ad: bool,

    /// Enable debug logging
    #[arg(short, long)]
    debug: bool,

    /// Twitch channel to watch (can also be twitch.tv/channel for Streamlink compatibility)
    channel: String,

    /// Stream quality/variant playlist to fetch (best, 1080p, 720p, 360p, 160p, audio_only)
    quality: String,
}

fn spawn_player_or_stdout(
    player_path: &Option<String>,
    player_args: &str,
) -> Result<Box<dyn Write + Send>> {
    if let Some(player_path) = player_path {
        info!("Opening player: {} {}", player_path, player_args);
        Ok(Box::new(
            Command::new(player_path)
                .args(player_args.split_whitespace())
                .stdin(Stdio::piped())
                .spawn()
                .context("Failed to open player")?
                .stdin
                .take()
                .context("Failed to open player stdin")?,
        ))
    } else {
        ensure!(
            !io::stdout().is_terminal(),
            "No player set and stdout is a terminal, exiting..."
        );

        info!("Writing to stdout");
        Ok(Box::new(io::stdout()))
    }
}

fn reload_loop(
    io_thread: &IOThread,
    playlist: &mut MediaPlaylist,
    disable_reset_on_ad: bool,
) -> Result<()> {
    loop {
        let time = Instant::now();

        let reload = playlist.reload()?;
        if !disable_reset_on_ad && reload.ad {
            warn!("Encountered an embedded ad segment, resetting");
            return Ok(()); //TODO: Use fallback servers
        } else if reload.discontinuity {
            warn!("Encountered a discontinuity, stream may be broken");
        }

        debug!("Playlist reload took {:?}", time.elapsed());

        match reload.sequence.cmp(&reload.prev_sequence) {
            Ordering::Greater => {
                io_thread.send_url(&reload.url)?;

                debug!("Sequence: {} -> {}", reload.prev_sequence, reload.sequence);
                debug!("Segment duration: {:?}", reload.delta);

                let elapsed = time.elapsed();
                if elapsed < reload.delta {
                    let sleep_time = reload.delta - elapsed;

                    debug!("Sleeping for {:?}", sleep_time);
                    thread::sleep(sleep_time);
                } else {
                    warn!("Took longer than segment duration, stream may be broken");
                }
            }
            Ordering::Less => bail!("Out of order media sequence"),
            Ordering::Equal => debug!("Sequence {} is the same as previous", reload.sequence), //try again immediately
        }
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
    if args.debug {
        TermLogger::init(
            LevelFilter::Debug,
            ConfigBuilder::new()
                .add_filter_ignore_str("ureq::unit")
                .add_filter_ignore_str("ureq::pool")
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

    let agent = AgentBuilder::new().user_agent(USER_AGENT).build();
    loop {
        let io_thread = IOThread::new(
            &agent,
            spawn_player_or_stdout(&args.player_path, &args.player_args)?,
        )?;

        let mut playlist = MediaPlaylist::new(&agent, &args.server, &args.channel, &args.quality)?;
        playlist.catch_up()?;

        if let Err(e) = reload_loop(&io_thread, &mut playlist, args.disable_reset_on_ad) {
            match e.downcast_ref::<ureq::Error>() {
                Some(ureq::Error::Status(code, _)) if *code == 404 => {
                    info!("Playlist not found. Stream likely ended, exiting...");
                    return Ok(());
                }
                _ => bail!(e),
            }
        }
    }
}
