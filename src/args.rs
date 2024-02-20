use std::{env, error::Error, fmt::Display, fs, path::Path, process, str::FromStr};

use anyhow::{Context, Result};
use pico_args::Arguments;

use crate::{
    constants, hls::Args as HlsArgs, http::Args as HttpArgs, player::Args as PlayerArgs,
    recorder::Args as RecorderArgs,
};

pub trait ArgParser {
    fn parse(&mut self, parser: &mut Parser) -> Result<()>;
}

#[derive(Default, Debug)]
pub struct Args {
    pub http: HttpArgs,
    pub player: PlayerArgs,
    pub recorder: RecorderArgs,
    pub hls: HlsArgs,
    pub debug: bool,
    pub passthrough: bool,
    pub print_streams: bool,
    pub quality: Option<String>,
}

impl ArgParser for Args {
    fn parse(&mut self, parser: &mut Parser) -> Result<()> {
        parser.parse_switch_or(&mut self.debug, "-d", "--debug")?;
        parser.parse_switch(&mut self.passthrough, "--passthrough")?;
        parser.parse_switch(&mut self.print_streams, "--print-streams")?;

        self.http.parse(parser)?;
        self.player.parse(parser)?;
        self.recorder.parse(parser)?;
        self.hls.parse(parser)?;

        if !self.print_streams {
            parser.parse_free(&mut self.quality, "quality")?;
        }

        Ok(())
    }
}

impl Args {
    pub fn new() -> Result<Self> {
        let mut parser = Parser::new()?;
        let mut args = Self::default();
        args.parse(&mut parser)?;

        Ok(args)
    }
}

pub struct Parser {
    parser: Arguments,
    config: Option<String>,
}

impl Parser {
    pub fn parse<T: FromStr>(&mut self, dst: &mut T, key: &'static str) -> Result<()>
    where
        <T as FromStr>::Err: Display + Send + Sync + Error + 'static,
    {
        let arg = self.parser.opt_value_from_str(key)?;
        Ok(self.resolve(dst, arg, key, T::from_str)?)
    }

    pub fn parse_cfg<T: FromStr>(
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

    pub fn parse_free(&mut self, dst: &mut Option<String>, cfg_key: &'static str) -> Result<()> {
        let arg = self.parser.opt_free_from_fn(Self::parse_opt_string)?;
        self.resolve(dst, arg, cfg_key, Self::parse_opt_string)
    }

    pub fn parse_free_required<T: FromStr>(&mut self) -> Result<T>
    where
        <T as FromStr>::Err: Display + Send + Sync + Error + 'static,
    {
        Ok(self.parser.free_from_str()?)
    }

    pub fn parse_switch(&mut self, dst: &mut bool, key: &'static str) -> Result<()> {
        let arg = self.parser.contains(key).then_some(true);
        Ok(self.resolve(dst, arg, key, bool::from_str)?)
    }

    pub fn parse_switch_or(
        &mut self,
        dst: &mut bool,
        key1: &'static str,
        key2: &'static str,
    ) -> Result<()> {
        let arg = (self.parser.contains(key1) || self.parser.contains(key2)).then_some(true);
        Ok(self.resolve(dst, arg, key2, bool::from_str)?)
    }

    pub fn parse_fn<T>(
        &mut self,
        dst: &mut T,
        key: &'static str,
        f: fn(_: &str) -> Result<T>,
    ) -> Result<()> {
        let arg = self.parser.opt_value_from_fn(key, f)?;
        self.resolve(dst, arg, key, f)
    }

    pub fn parse_fn_cfg<T>(
        &mut self,
        dst: &mut T,
        key: &'static str,
        cfg_key: &'static str,
        f: fn(_: &str) -> Result<T>,
    ) -> Result<()> {
        let arg = self.parser.opt_value_from_fn(key, f)?;
        self.resolve(dst, arg, cfg_key, f)
    }

    #[allow(clippy::unnecessary_wraps)] //function pointer
    pub fn parse_opt_string(arg: &str) -> Result<Option<String>> {
        Ok(Some(arg.to_owned()))
    }

    fn resolve<T, E>(
        &self,
        dst: &mut T,
        val: Option<T>,
        key: &'static str,
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

    fn new() -> Result<Self> {
        let mut parser = Arguments::from_env();
        if parser.contains("-h") || parser.contains("--help") {
            println!(include_str!("usage"));
            process::exit(0);
        }

        if parser.contains("-V") || parser.contains("--version") {
            println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"),);
            process::exit(0);
        }

        Ok(Self {
            config: {
                if parser.contains("--no-config") {
                    None
                } else {
                    let path = match parser.opt_value_from_str("-c")? {
                        Some(path) => path,
                        None => Self::default_config_path()?,
                    };

                    if Path::new(&path).try_exists()? {
                        Some(fs::read_to_string(path).context("Failed to read config file")?)
                    } else {
                        None
                    }
                }
            },
            parser,
        })
    }
}
