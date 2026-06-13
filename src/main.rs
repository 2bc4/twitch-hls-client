mod config;
mod constants;
mod hls;
mod http;
mod logger;
mod output;

use std::io;

use anyhow::Result;
use log::{debug, info};

use config::Config;
use hls::{Handler, OfflineError, Playlist, Stream};
use logger::Logger;
use output::{Player, PlayerClosedError, Writer};

fn main() -> Result<()> {
    Config::init()?;
    Logger::init()?;
    debug!("\n{:#?}", Config::get());

    let mut handler = {
        let conn = match Stream::new() {
            Ok(Stream::Variant(conn)) => conn,
            Ok(Stream::Passthrough(url)) => {
                return Player::passthrough(url);
            }
            Ok(Stream::None) => return Ok(()),
            Err(e) if e.is::<OfflineError>() => {
                info!("{e}, exiting...");
                return Ok(());
            }
            Err(e) => return Err(e),
        };

        Handler::new(Writer::new()?, Playlist::new(conn)?)?
    };

    let error = handler.run().expect_err("Handler returned Ok");
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
