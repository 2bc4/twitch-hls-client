use std::{
    env, fs,
    path::{Path, PathBuf},
    process,
};

use anyhow::{bail, Result};
use pico_args::Arguments;

use crate::constants;

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
    pub never_proxy: Option<Vec<String>>,
    pub channel: String,
    pub quality: String,

    servers_raw: Option<String>,
    never_proxy_raw: Option<String>,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            servers: Option::default(),
            player_path: PathBuf::default(),
            player_args: String::from("-"),
            debug: bool::default(),
            max_retries: 50,
            passthrough: bool::default(),
            client_id: Option::default(),
            auth_token: Option::default(),
            never_proxy: Option::default(),
            channel: String::default(),
            quality: String::default(),

            servers_raw: Option::default(),
            never_proxy_raw: Option::default(),
        }
    }
}

impl Args {
    pub fn parse() -> Result<Self> {
        let mut args = Self::default();
        let mut parser = Arguments::from_env();

        let config_path = if let Some(path) = parser.opt_value_from_str("-c")? {
            path
        } else {
            default_config_path()?
        };
        args.parse_config(&config_path)?;

        if parser.contains("-h") || parser.contains("--help") {
            eprintln!(include_str!("usage"));
            process::exit(0);
        }

        if parser.contains("-V") || parser.contains("--version") {
            eprintln!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
            process::exit(0);
        }

        merge_opt_val::<String>(&mut args.player_args, parser.opt_value_from_str("-a")?);
        merge_opt_val::<u32>(&mut args.max_retries, parser.opt_value_from_str("--max-retries")?);

        merge_opt_arg::<String>(&mut args.client_id, parser.opt_value_from_str("--client-id")?);
        merge_opt_arg::<String>(&mut args.auth_token, parser.opt_value_from_str("--auth-token")?);
        merge_opt_arg::<String>(
            &mut args.never_proxy_raw,
            parser.opt_value_from_str("--never-proxy")?,
        );
        args.parse_never_proxy();

        merge_switch(&mut args.passthrough, parser.contains("--passthrough"));
        merge_switch(
            &mut args.debug,
            parser.contains("-d") || parser.contains("--debug"),
        );

        if args.passthrough {
            parser.opt_value_from_str::<&str, String>("-p")?; //consume player arg
            args.finish(&mut parser)?;
            return Ok(args);
        }

        merge_opt_val::<PathBuf>(&mut args.player_path, parser.opt_value_from_str("-p")?);
        if args.player_path.to_string_lossy().is_empty() {
            bail!("player (-p) must be set");
        }

        args.finish(&mut parser)?;
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
                    "servers" => self.servers_raw = Some(split.1.into()),
                    "player" => self.player_path = split.1.into(),
                    "player-args" => self.player_args = split.1.into(),
                    "debug" => self.debug = split.1.parse()?,
                    "max-retries" => self.max_retries = split.1.parse()?,
                    "passthrough" => self.passthrough = split.1.parse()?,
                    "client-id" => self.client_id = Some(split.1.into()),
                    "auth-token" => self.auth_token = Some(split.1.into()),
                    "never-proxy" => self.never_proxy_raw = Some(split.1.into()),
                    _ => bail!("Unknown key in config: {}", split.0),
                }
            } else {
                bail!("Malformed config");
            }
        }

        Ok(())
    }

    fn parse_never_proxy(&mut self) {
        if let Some(never_proxy) = &self.never_proxy_raw {
            self.never_proxy = Some(split_comma(never_proxy));
            self.never_proxy_raw = Option::default();
        }
    }

    fn parse_servers(&mut self) {
        if let Some(never_proxy) = &self.never_proxy {
            if never_proxy.iter().any(|channel| channel.eq(&self.channel)) {
                return;
            }
        }

        if let Some(servers) = &self.servers_raw {
            self.servers = Some(split_comma(&servers.replace("[channel]", &self.channel)));
            self.servers_raw = Option::default();
        }
    }

    fn finish(&mut self, parser: &mut Arguments) -> Result<()> {
        merge_opt_arg::<String>(
            &mut self.servers_raw,
            parser.opt_value_from_str::<&str, String>("-s")?,
        );

        self.channel = parser
            .free_from_str::<String>()?
            .to_lowercase()
            .replace("twitch.tv/", "");
        self.quality = parser.free_from_str::<String>()?;

        self.parse_servers();

        Ok(())
    }
}

fn merge_opt_val<T>(dst: &mut T, val: Option<T>) {
    if let Some(val) = val {
        *dst = val;
    }
}

fn merge_switch(dst: &mut bool, val: bool) {
    if val {
        *dst = true;
    }
}

fn merge_opt_arg<T>(dst: &mut Option<T>, val: Option<T>) {
    if val.is_some() {
        *dst = val;
    }
}

fn split_comma(arg: &str) -> Vec<String> {
    arg.split(',').map(String::from).collect()
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
