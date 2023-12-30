use std::{env, fs, path::Path, process};

use anyhow::{bail, Result};
use pico_args::Arguments;

use crate::constants;

#[derive(Debug)]
#[allow(clippy::struct_field_names)]
pub struct Args {
    pub servers: Option<Vec<String>>,
    pub player: String,
    pub player_args: String,
    pub debug: bool,
    pub quiet: bool,
    pub max_retries: u32,
    pub passthrough: bool,
    pub client_id: Option<String>,
    pub auth_token: Option<String>,
    pub never_proxy: Option<Vec<String>>,
    pub http_retries: u32,
    pub http_connect_timeout: u64,
    pub channel: String,
    pub quality: String,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            servers: Option::default(),
            player: String::default(),
            player_args: String::from("-"),
            debug: bool::default(),
            quiet: bool::default(),
            max_retries: 50,
            passthrough: bool::default(),
            client_id: Option::default(),
            auth_token: Option::default(),
            never_proxy: Option::default(),
            http_retries: 3,
            http_connect_timeout: 5,
            channel: String::default(),
            quality: String::default(),
        }
    }
}

impl Args {
    pub fn parse() -> Result<Self> {
        let mut args = Self::default();
        let mut parser = Arguments::from_env();
        if parser.contains("-h") || parser.contains("--help") {
            eprintln!(include_str!("usage"));
            process::exit(0);
        }

        if parser.contains("-V") || parser.contains("--version") {
            eprintln!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
            process::exit(0);
        }

        let config_path = if let Some(path) = parser.opt_value_from_str("-c")? {
            path
        } else {
            default_config_path()?
        };
        args.parse_config(&config_path)?;

        args.merge_args(&mut parser)?;
        if args.passthrough {
            return Ok(args);
        }

        if args.player.is_empty() {
            bail!("player must be set");
        }

        Ok(args)
    }

    fn parse_config(&mut self, path: &str) -> Result<()> {
        if !Path::new(path).is_file() {
            return Ok(());
        }

        let config = fs::read_to_string(path)?;
        for line in config.lines() {
            if line.starts_with('#') {
                continue;
            }

            let split = line.split_once('=');
            if let Some(split) = split {
                match split.0 {
                    "servers" => self.servers = Some(split_comma(split.1)?),
                    "player" => self.player = split.1.into(),
                    "player-args" => self.player_args = split.1.into(),
                    "debug" => self.debug = split.1.parse()?,
                    "quiet" => self.quiet = split.1.parse()?,
                    "max-retries" => self.max_retries = split.1.parse()?,
                    "passthrough" => self.passthrough = split.1.parse()?,
                    "client-id" => self.client_id = Some(split.1.into()),
                    "auth-token" => self.auth_token = Some(split.1.into()),
                    "never-proxy" => self.never_proxy = Some(split_comma(split.1)?),
                    "http-retries" => self.http_retries = split.1.parse()?,
                    "http-connect-timeout" => self.http_connect_timeout = split.1.parse()?,
                    "quality" => self.quality = split.1.into(),
                    _ => bail!("Unknown key in config: {}", split.0),
                }
            } else {
                bail!("Malformed config");
            }
        }

        Ok(())
    }

    fn merge_args(&mut self, parser: &mut Arguments) -> Result<()> {
        merge_opt::<String>(&mut self.player, parser.opt_value_from_str("-p")?);
        merge_opt::<String>(&mut self.player_args, parser.opt_value_from_str("-a")?);
        merge_opt::<u32>(&mut self.max_retries, parser.opt_value_from_str("--max-retries")?);
        merge_opt::<u32>(&mut self.http_retries, parser.opt_value_from_str("--http-retries")?);
        merge_opt::<u64>(&mut self.http_connect_timeout, parser.opt_value_from_str("--http-connect-timeout")?);

        merge_opt_opt::<String>(&mut self.client_id, parser.opt_value_from_str("--client-id")?);
        merge_opt_opt::<String>(&mut self.auth_token, parser.opt_value_from_str("--auth-token")?);
        merge_opt_opt::<Vec<String>>(
            &mut self.never_proxy,
            parser.opt_value_from_fn("--never-proxy", split_comma)?,
        );
        merge_opt_opt::<Vec<String>>(&mut self.servers, parser.opt_value_from_fn("-s", split_comma)?);

        merge_switch(&mut self.passthrough, parser.contains("--passthrough"));
        merge_switch(
            &mut self.debug,
            parser.contains("-d") || parser.contains("--debug"),
        );
        merge_switch(
            &mut self.quiet,
            parser.contains("-q") || parser.contains("--quiet"),
        );

        self.channel = parser
            .free_from_str::<String>()?
            .to_lowercase()
            .replace("twitch.tv/", "");

        merge_opt::<String>(&mut self.quality, parser.opt_free_from_str()?);
        if self.quality.is_empty() {
            bail!("quality must be set");
        }

        if let Some(never_proxy) = &self.never_proxy {
            if never_proxy.iter().any(|a| a.eq(&self.channel)) {
                self.servers = None;
            }
        }

        Ok(())
    }
}

fn merge_opt<T>(dst: &mut T, val: Option<T>) {
    if let Some(val) = val {
        *dst = val;
    }
}

fn merge_opt_opt<T>(dst: &mut Option<T>, val: Option<T>) {
    if val.is_some() {
        *dst = val;
    }
}

fn merge_switch(dst: &mut bool, val: bool) {
    if val {
        *dst = true;
    }
}

#[allow(clippy::unnecessary_wraps)] //function pointer
fn split_comma(arg: &str) -> Result<Vec<String>> {
    Ok(arg.split(',').map(String::from).collect())
}

#[cfg(target_os = "linux")]
fn default_config_path() -> Result<String> {
    let dir = if let Ok(dir) = env::var("XDG_CONFIG_HOME") {
        dir
    } else {
        format!("{}/.config", env::var("HOME")?)
    };

    Ok(format!("{dir}/{}", constants::DEFAULT_CONFIG_PATH))
}

#[cfg(target_os = "windows")]
fn default_config_path() -> Result<String> {
    Ok(format!(
        "{}/{}",
        env::var("APPDATA")?,
        constants::DEFAULT_CONFIG_PATH,
    ))
}

#[cfg(target_os = "macos")]
fn default_config_path() -> Result<String> {
    //I have no idea if this is correct
    Ok(format!(
        "{}/Library/Application Support/{}",
        env::var("HOME")?,
        constants::DEFAULT_CONFIG_PATH,
    ))
}

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
fn default_config_path() -> Result<String> {
    Ok(constants::DEFAULT_CONFIG_PATH)
}
