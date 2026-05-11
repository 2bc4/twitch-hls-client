mod config;
mod constants;
mod hls;
mod http;
mod logger;
mod output;

use std::{io, time::Instant};

use anyhow::Result;
use log::{debug, info};

use config::Config;
use hls::{Handler, OfflineError, Playlist, ResetError, Stream};
use http::{Agent, Method};
use logger::Logger;
use output::{Output, Player, PlayerClosedError, Writer};

fn main_loop(mut writer: Writer, mut playlist: Playlist, agent: &Agent) -> Result<()> {
    if let Some(url) = &playlist.header {
        let mut request = agent.binary(Vec::new());
        request.call(Method::Get, url)?;

        writer.set_header(&request.into_writer())?;
    }

    if writer.should_wait() {
        writer.wait_for_output()?;
    }

    let mut handler = Handler::new(writer, agent)?;
    loop {
        let time = Instant::now();

        playlist.reload()?;
        if let Err(error) = handler.process(&mut playlist, time) {
            if error.is::<ResetError>() {
                playlist.reset();
                continue;
            }

            return Err(error);
        }
    }
}

fn main() -> Result<()> {
    Config::init()?;

    let (writer, playlist, agent) = {
        let cfg = Config::get();
        Logger::init(cfg.debug)?;
        debug!("\n{cfg:#?}");

        let agent = Agent::new();
        let conn = match Stream::new(&agent) {
            Ok(Stream::Variant(conn)) => conn,
            Ok(Stream::Passthrough(url)) => {
                return Player::passthrough(&url);
            }
            Ok(Stream::None) => return Ok(()),
            Err(e) if e.is::<OfflineError>() => {
                info!("{e}, exiting...");
                return Ok(());
            }
            Err(e) => return Err(e),
        };

        (Writer::new()?, Playlist::new(conn)?, agent)
    };

    let error = main_loop(writer, playlist, &agent).expect_err("Main loop returned Ok");
    if error.is::<OfflineError>() {
        info!("Stream ended, exiting...");
        return Ok(());
    }

    if let Some(error) = error.downcast_ref::<io::Error>().and_then(|e| e.get_ref())
        && error.is::<PlayerClosedError>()
    {
        info!("Player closed, exiting...");
        return Ok(());
    }

    Err(error)
}
