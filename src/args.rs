use std::{path::PathBuf, process};

use anyhow::Result;
use pico_args::Arguments;

#[derive(Default, Debug)]
pub struct Args {
    pub servers: Vec<String>,
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

        let channel = parser
            .free_from_str::<String>()?
            .to_lowercase()
            .replace("twitch.tv/", "");

        let mut args = Self {
            servers: parser
                .value_from_str::<&str, String>("-s")?
                .replace("[channel]", &channel)
                .split(',')
                .map(String::from)
                .collect(),
            player_path: PathBuf::default(),
            player_args: parser
                .opt_value_from_str("-a")?
                .unwrap_or_else(|| DEFAULT_PLAYER_ARGS.to_owned()),
            debug: parser.contains("-d") || parser.contains("--debug"),
            max_retries: parser
                .opt_value_from_str("--max-retries")?
                .unwrap_or(DEFAULT_MAX_RETRIES),
            passthrough: parser.contains("--passthrough"),
            channel,
            quality: parser.free_from_str::<String>()?,
        };

        if args.passthrough {
            return Ok(args);
        }

        args.player_path = parser.value_from_str("-p")?;
        args.player_args += &match args.player_path.file_stem() {
            Some(f) if f == "mpv" => format!(" --force-media-title=twitch.tv/{}", args.channel),
            Some(f) if f == "vlc" => format!(" --input-title-format=twitch.tv/{}", args.channel),
            _ => String::default(),
        };

        Ok(args)
    }
}
