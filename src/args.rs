use std::{env, fs, path::Path, process, time::Duration};

use anyhow::{bail, ensure, Context, Result};
use pico_args::Arguments;

use crate::constants;

#[derive(Debug)]
#[allow(clippy::module_name_repetitions)]
pub struct HttpArgs {
    pub force_https: bool,
    pub force_ipv4: bool,
    pub retries: u64,
    pub timeout: Duration,
    pub user_agent: String,
}

impl Default for HttpArgs {
    fn default() -> Self {
        Self {
            retries: 3,
            timeout: Duration::from_secs(10),
            force_https: bool::default(),
            force_ipv4: bool::default(),
            user_agent: constants::USER_AGENT.to_owned(),
        }
    }
}

#[derive(Debug)]
#[allow(clippy::module_name_repetitions)]
pub struct HlsArgs {
    pub codecs: String,
    pub channel: String,
    pub quality: String,
}

impl Default for HlsArgs {
    fn default() -> Self {
        Self {
            codecs: "av1,h265,h264".to_owned(),
            channel: String::default(),
            quality: String::default(),
        }
    }
}

#[derive(Clone, Debug)]
#[allow(clippy::module_name_repetitions)]
pub struct PlayerArgs {
    pub path: String,
    pub args: String,
    pub quiet: bool,
    pub no_kill: bool,
}

impl Default for PlayerArgs {
    fn default() -> Self {
        Self {
            args: "-".to_owned(),
            path: String::default(),
            quiet: bool::default(),
            no_kill: bool::default(),
        }
    }
}

#[derive(Default, Debug)]
pub struct Args {
    pub hls: HlsArgs,
    pub player: PlayerArgs,
    pub servers: Option<Vec<String>>,
    pub debug: bool,
    pub passthrough: bool,
    pub client_id: Option<String>,
    pub auth_token: Option<String>,
    pub never_proxy: Option<Vec<String>>,
}

impl Args {
    pub fn parse() -> Result<(Self, HttpArgs)> {
        let mut args = Self::default();
        let mut http_args = HttpArgs::default();
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

            args.parse_config(&mut http_args, &config_path)?;
        }

        args.merge_cli(&mut parser, &mut http_args)?;
        if let Some(never_proxy) = &args.never_proxy {
            if never_proxy.iter().any(|a| a.eq(&args.hls.channel)) {
                args.servers = None;
            }
        }

        ensure!(!args.player.path.is_empty(), "Player must be set");
        ensure!(!args.hls.quality.is_empty(), "Quality must be set");
        Ok((args, http_args))
    }

    fn parse_config(&mut self, http: &mut HttpArgs, path: &str) -> Result<()> {
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
                    "player" => self.player.path = split.1.into(),
                    "player-args" => self.player.args = split.1.into(),
                    "debug" => self.debug = split.1.parse()?,
                    "quiet" => self.player.quiet = split.1.parse()?,
                    "passthrough" => self.passthrough = split.1.parse()?,
                    "no-kill" => self.player.no_kill = split.1.parse()?,
                    "force-https" => http.force_https = split.1.parse()?,
                    "force-ipv4" => http.force_ipv4 = split.1.parse()?,
                    "client-id" => self.client_id = Some(split.1.into()),
                    "auth-token" => self.auth_token = Some(split.1.into()),
                    "never-proxy" => self.never_proxy = Some(split_comma(split.1)?),
                    "codecs" => self.hls.codecs = split.1.into(),
                    "user-agent" => http.user_agent = split.1.into(),
                    "http-retries" => http.retries = split.1.parse()?,
                    "http-timeout" => http.timeout = parse_duration(split.1)?,
                    "quality" => self.hls.quality = split.1.into(),
                    _ => bail!("Unknown key in config: {}", split.0),
                }
            } else {
                bail!("Malformed config");
            }
        }

        Ok(())
    }

    fn merge_cli(&mut self, p: &mut Arguments, http: &mut HttpArgs) -> Result<()> {
        merge_opt_opt(&mut self.servers, p.opt_value_from_fn("-s", split_comma)?);
        merge_opt(&mut self.player.path, p.opt_value_from_str("-p")?);
        merge_opt(&mut self.player.args, p.opt_value_from_str("-a")?);
        merge_switch(&mut self.debug, p.contains("-d") || p.contains("--debug"));
        merge_switch(
            &mut self.player.quiet,
            p.contains("-q") || p.contains("--quiet"),
        );
        merge_switch(&mut self.passthrough, p.contains("--passthrough"));
        merge_switch(&mut self.player.no_kill, p.contains("--no-kill"));
        merge_switch(&mut http.force_https, p.contains("--force-https"));
        merge_switch(&mut http.force_ipv4, p.contains("--force-ipv4"));
        merge_opt_opt(&mut self.client_id, p.opt_value_from_str("--client-id")?);
        merge_opt_opt(&mut self.auth_token, p.opt_value_from_str("--auth-token")?);
        merge_opt_opt(
            &mut self.never_proxy,
            p.opt_value_from_fn("--never-proxy", split_comma)?,
        );
        merge_opt(&mut http.user_agent, p.opt_value_from_str("--user-agent")?);
        merge_opt(&mut self.hls.codecs, p.opt_value_from_str("--codecs")?);
        merge_opt(&mut http.retries, p.opt_value_from_str("--http-retries")?);
        merge_opt(
            &mut http.timeout,
            p.opt_value_from_fn("--http-timeout", parse_duration)?,
        );

        self.hls.channel = p
            .free_from_str::<String>()
            .context("missing channel argument")?
            .to_lowercase()
            .replace("twitch.tv/", "");

        merge_opt(&mut self.hls.quality, p.opt_free_from_str()?);

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
