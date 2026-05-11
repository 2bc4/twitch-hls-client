use std::{
    borrow::Cow, env, error::Error, fmt::Display, fs, path::Path, process, str::FromStr,
    time::Duration,
};

use anyhow::{Context, Result, bail};

use crate::{
    Args as MainArgs, constants, hls::Args as HlsArgs, http::Args as HttpArgs,
    output::Args as OutputArgs,
};

pub trait Parse {
    fn parse(&mut self, parser: &mut Parser) -> Result<()>;
}

pub fn parse() -> Result<(MainArgs, HttpArgs, HlsArgs, OutputArgs)> {
    let mut main = MainArgs::default();
    let mut http = HttpArgs::default();
    let mut output = OutputArgs::default();
    let mut hls = HlsArgs::default();

    let mut parser = Parser::new()?;

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
    args: Arguments,
    config: Option<String>,
}

impl Parser {
    pub fn parse<T: FromStr>(&mut self, dst: &mut T, key: &'static str) -> Result<()>
    where
        <T as FromStr>::Err: Display + Send + Sync + Error + 'static,
    {
        let arg = self.take_parsed(key, T::from_str)?;
        Ok(self.resolve(dst, arg, key, T::from_str)?)
    }

    pub fn parse_opt<T: FromStr>(&mut self, dst: &mut Option<T>, key: &'static str) -> Result<()>
    where
        <T as FromStr>::Err: Display + Send + Sync + Error + 'static,
    {
        self.parse_fn(dst, key, Self::opt_from_str)
    }

    pub fn parse_opt_cfg<T: FromStr>(
        &mut self,
        dst: &mut Option<T>,
        key: &'static str,
        cfg_key: &'static str,
    ) -> Result<()>
    where
        <T as FromStr>::Err: Display + Send + Sync + Error + 'static,
    {
        self.parse_fn_cfg(dst, key, cfg_key, Self::opt_from_str)
    }

    pub fn parse_free(&mut self, dst: &mut Option<String>, cfg_key: &'static str) -> Result<()> {
        let arg = self
            .args
            .take_free()
            .as_deref()
            .map(Self::opt_from_str)
            .transpose()?;

        self.resolve(dst, arg, cfg_key, Self::opt_from_str)
    }

    pub fn parse_free_required(&mut self) -> Result<String> {
        self.args.take_free().context("No free argument found")
    }

    pub fn parse_switch(&mut self, dst: &mut bool, key: &'static str) -> Result<()> {
        let arg = self.args.contains(key).then_some(true);
        Ok(self.resolve(dst, arg, key, bool::from_str)?)
    }

    pub fn parse_switch_or(
        &mut self,
        dst: &mut bool,
        key1: &'static str,
        key2: &'static str,
    ) -> Result<()> {
        let arg = (self.args.contains(key1) | self.args.contains(key2)).then_some(true);
        Ok(self.resolve(dst, arg, key2, bool::from_str)?)
    }

    pub fn parse_fn<T>(
        &mut self,
        dst: &mut T,
        key: &'static str,
        f: fn(&str) -> Result<T>,
    ) -> Result<()> {
        let arg = self.take_parsed(key, f)?;
        self.resolve(dst, arg, key, f)
    }

    pub fn parse_fn_cfg<T>(
        &mut self,
        dst: &mut T,
        key: &'static str,
        cfg_key: &'static str,
        f: fn(&str) -> Result<T>,
    ) -> Result<()> {
        let arg = self.take_parsed(key, f)?;
        self.resolve(dst, arg, cfg_key, f)
    }

    /* These types should eventually just be wrapped with a FromStr impl */

    pub fn parse_cow_string(
        &mut self,
        dst: &mut Cow<'static, str>,
        key: &'static str,
    ) -> Result<()> {
        let arg = self.take_parsed(key, Self::cow_string_impl)?;
        self.resolve(dst, arg, key, Self::cow_string_impl)
    }

    pub fn parse_cow_string_cfg(
        &mut self,
        dst: &mut Cow<'static, str>,
        key: &'static str,
        cfg_key: &'static str,
    ) -> Result<()> {
        let arg = self.take_parsed(key, Self::cow_string_impl)?;
        self.resolve(dst, arg, cfg_key, Self::cow_string_impl)
    }

    pub fn parse_duration(&mut self, dst: &mut Duration, key: &'static str) -> Result<()> {
        let f = |a: &str| Ok(Duration::try_from_secs_f64(a.parse()?)?);
        let arg = self.take_parsed(key, f)?;
        self.resolve(dst, arg, key, f)
    }

    pub fn parse_comma_list<T: for<'a> From<&'a str>>(
        &mut self,
        dst: &mut Option<Vec<T>>,
        key: &'static str,
    ) -> Result<()> {
        self.parse_fn(dst, key, Self::comma_list_impl)
    }

    pub fn parse_comma_list_cfg<T: for<'a> From<&'a str>>(
        &mut self,
        dst: &mut Option<Vec<T>>,
        key: &'static str,
        cfg_key: &'static str,
    ) -> Result<()> {
        self.parse_fn_cfg(dst, key, cfg_key, Self::comma_list_impl)
    }

    fn take_parsed<T, E>(
        &mut self,
        key: &'static str,
        f: fn(&str) -> Result<T, E>,
    ) -> Result<Option<T>, E> {
        self.args.take_value(key).as_deref().map(f).transpose()
    }

    fn resolve<T, E>(
        &self,
        dst: &mut T,
        val: Option<T>,
        key: &'static str,
        f: fn(&str) -> Result<T, E>,
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

    fn opt_from_str<T: FromStr>(arg: &str) -> Result<Option<T>>
    where
        <T as FromStr>::Err: Display + Send + Sync + Error + 'static,
    {
        Ok(Some(arg.parse()?))
    }

    fn cow_string_impl(arg: &str) -> Result<Cow<'static, str>> {
        Ok(arg.to_owned().into())
    }

    fn comma_list_impl<T: for<'a> From<&'a str>>(arg: &str) -> Result<Option<Vec<T>>> {
        Ok(Some(arg.split(',').map(T::from).collect()))
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
        let mut args = Arguments::new();
        if args.contains("-h") | args.contains("--help") {
            print!(
                include_str!("usage"),
                default_user_agent = constants::USER_AGENT,
            );

            process::exit(0);
        }

        if args.contains("-V") | args.contains("--version") {
            println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
            process::exit(0);
        }

        Ok(Self {
            config: {
                if args.contains("--no-config") {
                    None
                } else {
                    let path = match args.take_value("-c") {
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
            args,
        })
    }

    fn finish(self) -> Option<String> {
        self.args.0.into_iter().next()
    }
}

struct Arguments(Vec<String>);

impl Arguments {
    fn new() -> Self {
        Self(
            env::args_os()
                .skip(1)
                .map(|a| a.to_string_lossy().into_owned())
                .collect(),
        )
    }

    fn contains(&mut self, key: &str) -> bool {
        if let Some(idx) = self.0.iter().position(|a| a == key) {
            self.0.remove(idx);
            true
        } else {
            false
        }
    }

    fn take_value(&mut self, key: &str) -> Option<String> {
        let idx = self.0.iter().position(|a| a == key)?;
        if idx + 1 < self.0.len() {
            self.0.remove(idx);
            Some(self.0.remove(idx))
        } else {
            None
        }
    }

    fn take_free(&mut self) -> Option<String> {
        let idx = self.0.iter().position(|a| !a.starts_with('-'))?;
        Some(self.0.remove(idx))
    }
}
