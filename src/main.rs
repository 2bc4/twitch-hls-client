mod args;
mod constants;
mod hls;
mod http;
mod logger;
mod output;

use std::{io, time::Instant};

use anyhow::Result;
use log::{debug, info};

use args::{Parse, Parser};
use hls::{Handler, OfflineError, Playlist, ResetError, Stream};
use http::{Agent, Method};
use logger::Logger;
use output::{Output, Player, PlayerClosedError, Writer};

#[derive(Default, Debug)]
pub struct Args {
    debug: bool,
}

impl Parse for Args {
    fn parse(&mut self, parser: &mut Parser) -> Result<()> {
        parser.parse_switch_or(&mut self.debug, "-d", "--debug")?;
        Ok(())
    }
}

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
    let (writer, playlist, agent) = {
        let (main_args, http_args, mut hls_args, mut output_args) = args::parse()?;

        Logger::init(main_args.debug)?;
        debug!("\n{main_args:#?}\n{http_args:#?}\n{hls_args:#?}\n{output_args:#?}");

        let agent = Agent::new(http_args);
        let conn = match Stream::new(&mut hls_args, &agent) {
            Ok(Stream::Variant(conn)) => conn,
            Ok(Stream::Passthrough(url)) => {
                return Player::passthrough(&mut output_args.player, &url, hls_args.channel());
            }
            Ok(Stream::Exit) => return Ok(()),
            Err(e) if e.is::<OfflineError>() => {
                info!("{e}, exiting...");
                return Ok(());
            }
            Err(e) => return Err(e),
        };

        (
            Writer::new(&output_args, hls_args.channel())?,
            Playlist::new(conn)?,
            agent,
        )
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
