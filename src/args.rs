use std::{
    borrow::Cow, env, error::Error, fmt::Display, fs, path::Path, process, str::FromStr,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use pico_args::Arguments;

use crate::{
    Args as MainArgs, constants, hls::Args as HlsArgs, http::Args as HttpArgs,
    output::Args as OutputArgs,
};

pub trait Parse {
    fn parse(&mut self, parser: &mut Parser) -> Result<()>;
}

pub fn parse() -> Result<(MainArgs, HttpArgs, HlsArgs, OutputArgs)> {
    let mut parser = Parser::new()?;

    let mut main = MainArgs::default();
    let mut http = HttpArgs::default();
    let mut hls = HlsArgs::default();
    let mut output = OutputArgs::default();

    main.parse(&mut parser)?;
    http.parse(&mut parser)?;
    output.parse(&mut parser)?;
    hls.parse(&mut parser)?; //must be last because it parses the free args

    if let Some(arg) = parser.finish() {
        bail!("Unrecognized argument: {arg}");
    }

    Ok((main, http, hls, output))
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

    pub fn parse_free(&mut self, dst: &mut Option<String>, cfg_key: &'static str) -> Result<()> {
        let arg = self.parser.opt_free_from_fn(Self::opt_string_impl)?;
        self.resolve(dst, arg, cfg_key, Self::opt_string_impl)
    }

    pub fn parse_free_required(&mut self) -> Result<String> {
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

    /* These types should eventually just be wrapped with a FromStr impl */

    pub fn parse_opt_string(&mut self, dst: &mut Option<String>, key: &'static str) -> Result<()> {
        let arg = self.parser.opt_value_from_fn(key, Self::opt_string_impl)?;
        self.resolve(dst, arg, key, Self::opt_string_impl)
    }

    pub fn parse_opt_string_cfg(
        &mut self,
        dst: &mut Option<String>,
        key: &'static str,
        cfg_key: &'static str,
    ) -> Result<()> {
        let arg = self.parser.opt_value_from_fn(key, Self::opt_string_impl)?;
        self.resolve(dst, arg, cfg_key, Self::opt_string_impl)
    }

    pub fn parse_cow_string(
        &mut self,
        dst: &mut Cow<'static, str>,
        key: &'static str,
    ) -> Result<()> {
        let arg = self.parser.opt_value_from_fn(key, Self::cow_string_impl)?;
        self.resolve(dst, arg, key, Self::cow_string_impl)
    }

    pub fn parse_cow_string_cfg(
        &mut self,
        dst: &mut Cow<'static, str>,
        key: &'static str,
        cfg_key: &'static str,
    ) -> Result<()> {
        let arg = self.parser.opt_value_from_fn(key, Self::cow_string_impl)?;
        self.resolve(dst, arg, cfg_key, Self::cow_string_impl)
    }

    pub fn parse_duration(&mut self, dst: &mut Duration, key: &'static str) -> Result<()> {
        let f = |a: &str| Ok(Duration::try_from_secs_f64(a.parse()?)?);

        let arg = self.parser.opt_value_from_fn(key, f)?;
        self.resolve(dst, arg, key, f)
    }

    fn resolve<T, E>(
        &self,
        dst: &mut T,
        val: Option<T>,
        key: &'static str,
        f: fn(_: &str) -> Result<T, E>,
    ) -> Result<(), E> {
        //unwrap arg or try to get arg from config file
        if let Some(val) = val {
            *dst = val;
        } else if let Some(cfg) = &self.config {
            let key = key.trim_start_matches('-');
            if let Some(val) = cfg
                .lines()
                .find(|l| l.starts_with(key))
                .and_then(|l| l.split_once('='))
                .and_then(|(k, v)| k.eq(key).then_some(v))
            {
                *dst = f(val)?;
            }
        }

        Ok(())
    }

    #[allow(clippy::unnecessary_wraps, reason = "function pointer")]
    fn opt_string_impl(arg: &str) -> Result<Option<String>> {
        Ok(Some(arg.to_owned()))
    }

    #[allow(clippy::unnecessary_wraps, reason = "function pointer")]
    fn cow_string_impl(arg: &str) -> Result<Cow<'static, str>> {
        Ok(arg.to_owned().into())
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
            print!(include_str!("usage"));
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

    fn finish(self) -> Option<String> {
        self.parser.finish().into_iter().next()?.into_string().ok()
    }
}
