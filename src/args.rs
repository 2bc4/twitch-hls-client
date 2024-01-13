use std::{env, fs, path::Path, process, time::Duration};

use anyhow::{bail, Context, Result};
use pico_args::Arguments;

use crate::constants;

#[derive(Debug)]
#[allow(clippy::struct_field_names)] //.player_args
#[allow(clippy::struct_excessive_bools)]
pub struct Args {
    pub servers: Option<Vec<String>>,
    pub player: String,
    pub player_args: String,
    pub debug: bool,
    pub quiet: bool,
    pub passthrough: bool,
    pub no_kill: bool,
    pub force_https: bool,
    pub force_ipv4: bool,
    pub client_id: Option<String>,
    pub auth_token: Option<String>,
    pub never_proxy: Option<Vec<String>>,
    pub codecs: String,
    pub http_retries: u64,
    pub http_timeout: Duration,
    pub channel: String,
    pub quality: String,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            servers: Option::default(),
            player: String::default(),
            player_args: "-".to_owned(),
            debug: bool::default(),
            quiet: bool::default(),
            passthrough: bool::default(),
            no_kill: bool::default(),
            force_https: bool::default(),
            force_ipv4: bool::default(),
            client_id: Option::default(),
            auth_token: Option::default(),
            never_proxy: Option::default(),
            codecs: "av1,h265,h264".to_owned(),
            http_retries: 3,
            http_timeout: Duration::from_secs(10),
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
            println!(include_str!("usage"));
            process::exit(0);
        }

        if parser.contains("-V") || parser.contains("--version") {
            println!(
                "{} {} (curl {})",
                env!("CARGO_PKG_NAME"),
                env!("CARGO_PKG_VERSION"),
                curl::Version::get().version(),
            );

            process::exit(0);
        }

        if !parser.contains("--no-config") {
            let config_path = match parser.opt_value_from_str("-c")? {
                Some(path) => path,
                None => default_config_path()?,
            };

            args.parse_config(&config_path)?;
        }

        args.merge_cli(&mut parser)?;
        if let Some(never_proxy) = &args.never_proxy {
            if never_proxy.iter().any(|a| a.eq(&args.channel)) {
                args.servers = None;
            }
        }

        if args.player.is_empty() {
            bail!("player must be set");
        }

        if args.quality.is_empty() {
            bail!("quality must be set");
        }

        Ok(args)
    }

    fn parse_config(&mut self, path: &str) -> Result<()> {
        if !Path::new(path).is_file() {
            return Ok(());
        }

        let config = fs::read_to_string(path).context("Failed to read config file")?;
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
                    "passthrough" => self.passthrough = split.1.parse()?,
                    "no-kill" => self.no_kill = split.1.parse()?,
                    "force-https" => self.force_https = split.1.parse()?,
                    "force-ipv4" => self.force_ipv4 = split.1.parse()?,
                    "client-id" => self.client_id = Some(split.1.into()),
                    "auth-token" => self.auth_token = Some(split.1.into()),
                    "never-proxy" => self.never_proxy = Some(split_comma(split.1)?),
                    "codecs" => self.codecs = split.1.into(),
                    "http-retries" => self.http_retries = split.1.parse()?,
                    "http-timeout" => self.http_timeout = parse_duration(split.1)?,
                    "quality" => self.quality = split.1.into(),
                    _ => bail!("Unknown key in config: {}", split.0),
                }
            } else {
                bail!("Malformed config");
            }
        }

        Ok(())
    }

    fn merge_cli(&mut self, p: &mut Arguments) -> Result<()> {
        merge_opt_opt(&mut self.servers, p.opt_value_from_fn("-s", split_comma)?);
        merge_opt(&mut self.player, p.opt_value_from_str("-p")?);
        merge_opt(&mut self.player_args, p.opt_value_from_str("-a")?);
        merge_switch(&mut self.debug, p.contains("-d") || p.contains("--debug"));
        merge_switch(&mut self.quiet, p.contains("-q") || p.contains("--quiet"));
        merge_switch(&mut self.passthrough, p.contains("--passthrough"));
        merge_switch(&mut self.no_kill, p.contains("--no-kill"));
        merge_switch(&mut self.force_https, p.contains("--force-https"));
        merge_switch(&mut self.force_ipv4, p.contains("--force-ipv4"));
        merge_opt_opt(&mut self.client_id, p.opt_value_from_str("--client-id")?);
        merge_opt_opt(&mut self.auth_token, p.opt_value_from_str("--auth-token")?);
        merge_opt_opt(
            &mut self.never_proxy,
            p.opt_value_from_fn("--never-proxy", split_comma)?,
        );
        merge_opt(&mut self.codecs, p.opt_value_from_str("--codecs")?);
        merge_opt(
            &mut self.http_retries,
            p.opt_value_from_str("--http-retries")?,
        );
        merge_opt(
            &mut self.http_timeout,
            p.opt_value_from_fn("--http-timeout", parse_duration)?,
        );

        self.channel = p
            .free_from_str::<String>()
            .context("missing channel argument")?
            .to_lowercase()
            .replace("twitch.tv/", "");

        merge_opt(&mut self.quality, p.opt_free_from_str()?);

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

fn parse_duration(arg: &str) -> Result<Duration> {
    Ok(Duration::try_from_secs_f64(arg.parse()?)?)
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
