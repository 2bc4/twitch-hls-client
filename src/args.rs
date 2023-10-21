use std::{path::PathBuf, process};

use anyhow::{bail, Result};
use pico_args::Arguments;

#[derive(Debug)]
pub struct Args {
    pub servers: Option<Vec<String>>,
    pub player_path: PathBuf,
    pub player_args: String,
    pub debug: bool,
    pub max_retries: u32,
    pub passthrough: bool,
    pub client_id: Option<String>,
    pub auth_token: Option<String>,
    pub channel: String,
    pub quality: String,
}

impl Args {
    pub fn parse() -> Result<Self> {
        const DEFAULT_PLAYER_ARGS: &str = "-";
        const DEFAULT_MAX_RETRIES: u32 = 50;

        let mut parser = Arguments::from_env();
        if parser.contains("-h") || parser.contains("--help") {
            eprintln!(include_str!("usage"));
            process::exit(0);
        }

        if parser.contains("-V") || parser.contains("--version") {
            eprintln!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
            process::exit(0);
        }

        let mut args = Self {
            servers: Option::default(),
            player_path: PathBuf::default(),
            player_args: parser
                .opt_value_from_str("-a")?
                .unwrap_or_else(|| DEFAULT_PLAYER_ARGS.to_owned()),
            debug: parser.contains("-d") || parser.contains("--debug"),
            max_retries: parser
                .opt_value_from_str("--max-retries")?
                .unwrap_or(DEFAULT_MAX_RETRIES),
            passthrough: parser.contains("--passthrough"),
            client_id: parser.opt_value_from_str("--client-id")?,
            auth_token: parser.opt_value_from_str("--auth-token")?,
            channel: String::default(),
            quality: String::default(),
        };

        if args.passthrough {
            parser.value_from_str::<&str, String>("-p")?; //consume player arg
            args.finish(&mut parser)?;
            return Ok(args);
        }

        args.player_path = parser.value_from_str("-p")?;
        args.finish(&mut parser)?;
        Ok(args)
    }

    fn finish(&mut self, parser: &mut Arguments) -> Result<()> {
        let servers = parser.opt_value_from_str::<&str, String>("-s")?;

        self.channel = parser
            .free_from_str::<String>()?
            .to_lowercase()
            .replace("twitch.tv/", "");
        self.quality = parser.free_from_str::<String>()?;

        if let Some(servers) = servers {
            self.servers = Some(
                servers
                    .replace("[channel]", &self.channel)
                    .split(',')
                    .map(String::from)
                    .collect(),
            );
        }

        if self.servers.is_some() && (self.client_id.is_some() || self.auth_token.is_some()) {
            bail!("Client ID or auth token cannot be set while using a playlist proxy");
        }

        Ok(())
    }
}
