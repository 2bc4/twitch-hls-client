use std::{path::PathBuf, process};

use anyhow::Result;
use pico_args::Arguments;

#[derive(Default, Debug)]
pub struct Args {
    pub servers: Option<Vec<String>>,
    pub player_path: PathBuf,
    pub player_args: String,
    pub debug: bool,
    pub max_retries: u32,
    pub passthrough: bool,
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
            player_path: parser.value_from_str("-p")?,
            player_args: parser
                .opt_value_from_str("-a")?
                .unwrap_or_else(|| DEFAULT_PLAYER_ARGS.to_owned()),
            debug: parser.contains("-d") || parser.contains("--debug"),
            max_retries: parser
                .opt_value_from_str("--max-retries")?
                .unwrap_or(DEFAULT_MAX_RETRIES),
            passthrough: parser.contains("--passthrough"),
            channel: parser
                .free_from_str::<String>()?
                .to_lowercase()
                .replace("twitch.tv/", ""),
            quality: parser.free_from_str::<String>()?,
        };

        let servers = parser.opt_value_from_str::<&str, String>("-s")?;
        if let Some(servers) = servers {
            args.servers = Some(
                servers
                    .replace("[channel]", &args.channel)
                    .split(',')
                    .map(String::from)
                    .collect(),
            );
        }

        if args.passthrough {
            return Ok(args);
        }

        args.player_args += &match args.player_path.file_stem() {
            Some(f) if f == "mpv" => format!(" --force-media-title=twitch.tv/{}", args.channel),
            Some(f) if f == "vlc" => format!(" --input-title-format=twitch.tv/{}", args.channel),
            _ => String::default(),
        };

        Ok(args)
    }
}
