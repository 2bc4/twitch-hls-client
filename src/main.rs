mod args;
mod constants;
mod hls;
mod http;
mod logger;
mod output;

use std::{
    io::{self, ErrorKind::Other},
    time::Instant,
};

use anyhow::Result;
use log::{debug, info};

use args::{Parse, Parser};
use hls::{Handler, MediaPlaylist, OfflineError};
use http::Agent;
use logger::Logger;
use output::{Player, Writer};

#[derive(Default, Debug)]
pub struct Args {
    debug: bool,
    passthrough: bool,
}

impl Parse for Args {
    fn parse(&mut self, parser: &mut Parser) -> Result<()> {
        parser.parse_switch_or(&mut self.debug, "-d", "--debug")?;
        parser.parse_switch(&mut self.passthrough, "--passthrough")?;

        Ok(())
    }
}

fn main_loop(mut handler: Handler, mut playlist: MediaPlaylist) -> Result<()> {
    handler.process(&mut playlist, Instant::now())?;
    loop {
        let time = Instant::now();

        playlist.reload()?;
        handler.process(&mut playlist, time)?;
    }
}

fn main() -> Result<()> {
    let (handler, playlist) = {
        let (main_args, http_args, hls_args, mut output_args) = args::parse()?;

        Logger::init(main_args.debug)?;
        debug!("\n{main_args:#?}\n{http_args:#?}\n{hls_args:#?}\n{output_args:#?}");

        let agent = Agent::new(http_args);
        let conn = match hls::fetch_playlist(hls_args, &agent) {
            Ok(Some(conn)) => conn,
            Ok(None) => return Ok(()),
            Err(e) if e.downcast_ref::<OfflineError>().is_some() => {
                info!("{e}, exiting...");
                return Ok(());
            }
            Err(e) => return Err(e),
        };

        if main_args.passthrough {
            return Player::passthrough(&mut output_args.player, &conn.url);
        }

        let mut playlist = MediaPlaylist::new(conn)?;
        let writer = Writer::new(&output_args)?;

        (Handler::new(writer, &mut playlist, agent)?, playlist)
    };

    match main_loop(handler, playlist) {
        Ok(()) => Ok(()),
        Err(e) if e.downcast_ref::<OfflineError>().is_some() => {
            info!("Stream ended, exiting...");
            Ok(())
        }
        Err(e)
            if e.downcast_ref::<io::Error>()
                .is_some_and(|e| e.kind() == Other) =>
        {
            info!("Player closed, exiting...");
            Ok(())
        }
        Err(e) => Err(e),
    }
}
