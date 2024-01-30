#![allow(clippy::unnecessary_wraps)] //function pointers

use std::{env, error::Error, fmt::Display, fs, path::Path, process, str::FromStr, time::Duration};

use anyhow::{ensure, Context, Result};
use pico_args::Arguments;

use crate::{constants, hls::Args as HlsArgs, http::Args as HttpArgs, player::Args as PlayerArgs};

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
        let mut p = Parser::new()?;
        let mut args = Self::default();
        let mut http = HttpArgs::default();

        p.parse_fn_cfg(&mut args.servers, "-s", "servers", split_comma)?;
        p.parse_cfg(&mut args.player.path, "-p", "player")?;
        p.parse_cfg(&mut args.player.args, "-a", "player-args")?;
        p.parse_switch_or(&mut args.debug, "-d", "--debug")?;
        p.parse_switch_or(&mut args.player.quiet, "-q", "--quiet")?;
        p.parse_switch(&mut args.passthrough, "--passthrough")?;
        p.parse_switch(&mut args.player.no_kill, "--no-kill")?;
        p.parse_switch(&mut http.force_https, "--force-https")?;
        p.parse_switch(&mut http.force_ipv4, "--force-ipv4")?;
        p.parse_fn(&mut args.client_id, "--client-id", parse_optstring)?;
        p.parse_fn(&mut args.auth_token, "--auth-token", parse_optstring)?;
        p.parse_fn(&mut args.never_proxy, "--never-proxy", split_comma)?;
        p.parse(&mut args.hls.codecs, "--codecs")?;
        p.parse(&mut http.user_agent, "--user-agent")?;
        p.parse(&mut http.retries, "--http-retries")?;
        p.parse_fn(&mut http.timeout, "--http-timeout", parse_duration)?;

        args.hls.channel = p
            .parser
            .free_from_str::<String>()
            .context("Missing channel argument")?
            .to_lowercase()
            .replace("twitch.tv/", "");

        p.parse_free(&mut args.hls.quality, "quality")?;

        if let Some(ref never_proxy) = args.never_proxy {
            if never_proxy.iter().any(|a| a.eq(&args.hls.channel)) {
                args.servers = None;
            }
        }

        let remaining = p.parser.finish();
        ensure!(remaining.is_empty(), "Invalid argument: {:?}", remaining);

        ensure!(!args.player.path.is_empty(), "Player must be set");
        ensure!(!args.hls.quality.is_empty(), "Quality must be set");
        Ok((args, http))
    }
}

fn split_comma(arg: &str) -> Result<Option<Vec<String>>> {
    Ok(Some(arg.split(',').map(String::from).collect()))
}

fn parse_duration(arg: &str) -> Result<Duration> {
    Ok(Duration::try_from_secs_f64(arg.parse()?)?)
}

fn parse_optstring(arg: &str) -> Result<Option<String>> {
    Ok(Some(arg.to_owned()))
}

struct Parser {
    parser: Arguments,
    config: Option<String>,
}

impl Parser {
    fn new() -> Result<Self> {
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

        Ok(Self {
            config: {
                if parser.contains("--no-config") {
                    None
                } else {
                    let path = match parser.opt_value_from_str("-c")? {
                        Some(path) => path,
                        None => default_config_path()?,
                    };

                    if Path::new(&path).exists() {
                        Some(fs::read_to_string(path).context("Failed to read config file")?)
                    } else {
                        None
                    }
                }
            },
            parser,
        })
    }

    fn parse<T: FromStr>(&mut self, dst: &mut T, key: &'static str) -> Result<()>
    where
        <T as FromStr>::Err: Display + Send + Sync + Error + 'static,
    {
        let arg = self.parser.opt_value_from_str(key)?;
        Ok(self.resolve(dst, arg, key, T::from_str)?)
    }

    fn parse_cfg<T: FromStr>(
        &mut self,
        dst: &mut T,
        key: &'static str,
        cfg_key: &'static str,
    ) -> Result<()>
    where
        <T as FromStr>::Err: Display + Send + Sync + Error + 'static,
    {
        let arg = self.parser.opt_value_from_str(key)?;
        Ok(self.resolve(dst, arg, cfg_key, T::from_str)?)
    }

    fn parse_free<T: FromStr>(&mut self, dst: &mut T, cfg_key: &'static str) -> Result<()>
    where
        <T as FromStr>::Err: Display + Send + Sync + Error + 'static,
    {
        let arg = self.parser.opt_free_from_str()?;
        Ok(self.resolve(dst, arg, cfg_key, T::from_str)?)
    }

    fn parse_switch(&mut self, dst: &mut bool, key: &'static str) -> Result<()> {
        let arg = Some(self.parser.contains(key));
        Ok(self.resolve(dst, arg, key, bool::from_str)?)
    }

    fn parse_switch_or(
        &mut self,
        dst: &mut bool,
        key1: &'static str,
        key2: &'static str,
    ) -> Result<()> {
        let arg = Some(self.parser.contains(key1) || self.parser.contains(key2));
        Ok(self.resolve(dst, arg, key2, bool::from_str)?)
    }

    fn parse_fn<T>(
        &mut self,
        dst: &mut T,
        key: &'static str,
        f: fn(_: &str) -> Result<T>,
    ) -> Result<()> {
        let arg = self.parser.opt_value_from_fn(key, f)?;
        self.resolve(dst, arg, key, f)
    }

    fn parse_fn_cfg<T>(
        &mut self,
        dst: &mut T,
        key: &'static str,
        cfg_key: &'static str,
        f: fn(_: &str) -> Result<T>,
    ) -> Result<()> {
        let arg = self.parser.opt_value_from_fn(key, f)?;
        self.resolve(dst, arg, cfg_key, f)
    }

    fn resolve<T, E>(
        &self,
        dst: &mut T,
        val: Option<T>,
        key: &str,
        f: fn(_: &str) -> Result<T, E>,
    ) -> Result<(), E> {
        //unwrap val or get arg from config file
        if let Some(val) = val {
            *dst = val;
        } else if let Some(ref cfg) = self.config {
            let key = key.trim_start_matches('-');
            if let Some(line) = cfg.lines().find(|l| l.starts_with(key)) {
                if let Some(split) = line.split_once('=') {
                    *dst = f(split.1)?;
                }
            }
        }

        Ok(())
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
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

#[cfg(not(any(unix, target_os = "windows", target_os = "macos")))]
fn default_config_path() -> Result<String> {
    Ok(constants::DEFAULT_CONFIG_PATH)
}
