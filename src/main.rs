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
use hls::{Handler, MediaPlaylist, OfflineError, ResetError};
use http::{Agent, Method};
use logger::Logger;
use output::{Output, Player, Writer};

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

fn main_loop(mut writer: Writer, mut playlist: MediaPlaylist, agent: Agent) -> Result<()> {
    if let Some(url) = playlist.header.take() {
        let mut request = agent.binary(Vec::new());
        request.call(Method::Get, &url)?;

        writer.set_header(&request.into_writer())?;
    }

    if writer.should_wait() {
        writer.wait_for_output()?;
    }

    let mut handler = Handler::new(writer, agent)?;
    loop {
        let time = Instant::now();

        playlist.reload()?;
        if let Err(e) = handler.process(&mut playlist, time) {
            if e.downcast_ref::<ResetError>().is_some() {
                playlist.reset();
                continue;
            }

            return Err(e);
        }
    }
}

fn main() -> Result<()> {
    let (writer, playlist, agent) = {
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

        (Writer::new(&output_args)?, MediaPlaylist::new(conn)?, agent)
    };

    match main_loop(writer, playlist, agent) {
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
