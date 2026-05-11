use std::{
    borrow::Cow,
    env,
    error::Error,
    fmt::{self, Display},
    fs,
    net::{SocketAddr, ToSocketAddrs},
    path::Path,
    process,
    str::FromStr,
    sync::OnceLock,
    time::Duration,
};

use crate::{constants, hls::Passthrough, http::Url};
use anyhow::{Context, Result, bail, ensure};

static CONFIG: OnceLock<Config> = OnceLock::new();

#[derive(Clone)]
pub struct Config {
    pub debug: bool,

    pub servers: Option<Vec<Url>>,
    pub print_streams: bool,
    pub no_low_latency: bool,
    pub passthrough: Passthrough,
    pub client_id: Option<String>,
    pub auth_token: Option<String>,
    pub codecs: Cow<'static, str>,
    pub never_proxy: Option<Vec<String>>,
    pub playlist_cache_dir: Option<String>,
    pub use_cache_only: bool,
    pub write_cache_only: bool,
    pub force_playlist_url: Option<Url>,
    pub channel: String,
    pub quality: Option<String>,

    pub player_path: Option<String>,
    pub player_args: Cow<'static, str>,
    pub player_quiet: bool,
    pub player_no_kill: bool,

    pub tcp_addr: Option<SocketAddr>,
    pub tcp_client_timeout: Duration,

    pub record_path: Option<String>,
    pub overwrite: bool,

    pub force_https: bool,
    pub force_ipv4: bool,
    pub retries: u64,
    pub timeout: Duration,
    pub user_agent: Cow<'static, str>,
    pub socks5: Option<Vec<SocketAddr>>,
    pub socks5_restrict: Option<Vec<String>>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            debug: bool::default(),
            servers: Option::default(),
            print_streams: bool::default(),
            no_low_latency: bool::default(),
            passthrough: Passthrough::default(),
            client_id: Option::default(),
            auth_token: Option::default(),
            codecs: "av1,h265,h264".into(),
            never_proxy: Option::default(),
            playlist_cache_dir: Option::default(),
            use_cache_only: bool::default(),
            write_cache_only: bool::default(),
            force_playlist_url: Option::default(),
            channel: String::default(),
            quality: Option::default(),
            player_path: Option::default(),
            player_args: "-".into(),
            player_quiet: bool::default(),
            player_no_kill: bool::default(),
            tcp_addr: Option::default(),
            tcp_client_timeout: Duration::from_secs(30),
            record_path: Option::default(),
            overwrite: bool::default(),
            force_https: bool::default(),
            force_ipv4: bool::default(),
            retries: 3,
            timeout: Duration::from_secs(10),
            user_agent: constants::USER_AGENT.into(),
            socks5: Option::default(),
            socks5_restrict: Option::default(),
        }
    }
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let hide =
            |opt: &Option<String>| -> Option<&'static str> { opt.as_ref().map(|_| "<hidden>") };

        f.debug_struct("Config")
            .field("debug", &self.debug)
            .field("servers", &self.servers)
            .field("print_streams", &self.print_streams)
            .field("no_low_latency", &self.no_low_latency)
            .field("passthrough", &self.passthrough)
            .field("client_id", &hide(&self.client_id))
            .field("auth_token", &hide(&self.auth_token))
            .field("codecs", &self.codecs)
            .field("never_proxy", &self.never_proxy)
            .field("playlist_cache_dir", &self.playlist_cache_dir)
            .field("use_cache_only", &self.use_cache_only)
            .field("write_cache_only", &self.write_cache_only)
            .field("force_playlist_url", &self.force_playlist_url)
            .field("channel", &self.channel)
            .field("quality", &self.quality)
            .field("player_path", &self.player_path)
            .field("player_args", &self.player_args)
            .field("player_quiet", &self.player_quiet)
            .field("player_no_kill", &self.player_no_kill)
            .field("tcp_addr", &self.tcp_addr)
            .field("tcp_client_timeout", &self.tcp_client_timeout)
            .field("record_path", &self.record_path)
            .field("overwrite", &self.overwrite)
            .field("force_https", &self.force_https)
            .field("force_ipv4", &self.force_ipv4)
            .field("retries", &self.retries)
            .field("timeout", &self.timeout)
            .field("user_agent", &self.user_agent)
            .field("socks5", &self.socks5)
            .field("socks5_restrict", &self.socks5_restrict)
            .finish()
    }
}

impl Config {
    const CHANNEL_KEYWORD: &str = "[channel]";

    pub fn init() -> Result<()> {
        CONFIG
            .set(Self::parse()?)
            .expect("Config already initialized");

        Ok(())
    }

    pub fn get() -> &'static Self {
        CONFIG.get().expect("Config not initialized")
    }

    pub const fn has_output(&self) -> bool {
        self.player_path.is_some() || self.tcp_addr.is_some() || self.record_path.is_some()
    }

    fn parse() -> Result<Self> {
        let mut cfg = Self::default();
        let mut parser = Parser::new()?;

        cfg.parse_switches(&mut parser)?;
        cfg.parse_values(&mut parser)?;
        cfg.parse_options(&mut parser)?;

        let channel = parser.free().context("Missing channel argument")?;

        //Accept any URL-like string
        cfg.channel = channel
            .rsplit_once('/')
            .map_or(channel.as_str(), |s| s.1)
            .to_lowercase();

        cfg.quality = parser.free();
        if cfg.print_streams {
            cfg.quality = None;
        }

        if cfg.use_cache_only || cfg.write_cache_only {
            ensure!(
                cfg.playlist_cache_dir.is_some(),
                "--playlist-cache-dir not configured"
            );

            ensure!(
                !(cfg.use_cache_only && cfg.write_cache_only),
                "--use-cache-only and --write-cache-only cannot be used together"
            );
        }

        if let Some(never_proxy) = &cfg.never_proxy
            && never_proxy.iter().any(|c| c.eq(&cfg.channel))
        {
            cfg.servers = None;
        }

        if cfg.player_args.contains(Self::CHANNEL_KEYWORD) {
            cfg.player_args = cfg
                .player_args
                .replace(Self::CHANNEL_KEYWORD, &cfg.channel)
                .into();
        }

        if let Some(servers) = &mut cfg.servers {
            for server in servers {
                if server.contains(Self::CHANNEL_KEYWORD) {
                    *server = server.replace(Self::CHANNEL_KEYWORD, &cfg.channel).into();
                }
            }
        }

        if let Some(arg) = parser.remaining() {
            bail!("Unrecognized argument: {arg}");
        }

        Ok(cfg)
    }

    fn parse_switches(&mut self, p: &mut Parser) -> Result<()> {
        p.switch(&mut self.debug, "-d", "--debug")?;
        p.switch(&mut self.print_streams, "", "--print-streams")?;
        p.switch(&mut self.no_low_latency, "", "--no-low-latency")?;
        p.switch(&mut self.use_cache_only, "", "--use-cache-only")?;
        p.switch(&mut self.write_cache_only, "", "--write-cache-only")?;
        p.switch(&mut self.player_quiet, "-q", "--quiet")?;
        p.switch(&mut self.player_no_kill, "", "--no-kill")?;
        p.switch(&mut self.overwrite, "", "--overwrite")?;
        p.switch(&mut self.force_https, "", "--force-https")?;
        p.switch(&mut self.force_ipv4, "", "--force-ipv4")?;

        Ok(())
    }

    fn parse_values(&mut self, p: &mut Parser) -> Result<()> {
        p.with(&mut self.servers, "-s", "servers", |s| {
            Ok(Some(
                s.split(',')
                    .filter_map(|u| Url::from_str(u.trim()).ok())
                    .collect(),
            ))
        })?;
        p.with(
            &mut self.passthrough,
            "--passthrough",
            "passthrough",
            Passthrough::new,
        )?;
        p.with(&mut self.codecs, "--codecs", "codecs", Self::cow)?;
        p.with(&mut self.never_proxy, "--never-proxy", "never-proxy", |c| {
            Ok(Some(
                c.split(',').map(|c| c.trim().to_lowercase()).collect(),
            ))
        })?;
        p.with(
            &mut self.force_playlist_url,
            "--force-playlist-url",
            "force-playlist-url",
            |u| Ok(Some(Url::from_str(u)?)),
        )?;
        p.with(&mut self.player_args, "-a", "player-args", Self::cow)?;
        p.with(&mut self.tcp_addr, "-t", "tcp-server", |s| {
            Ok(Some(
                s.to_socket_addrs()?
                    .next()
                    .context("Invalid socket address")?,
            ))
        })?;
        p.with(
            &mut self.tcp_client_timeout,
            "--tcp-client-timeout",
            "tcp-client-timeout",
            Self::duration,
        )?;
        p.with(&mut self.retries, "--http-retries", "http-retries", |s| {
            Ok(s.parse()?)
        })?;
        p.with(
            &mut self.timeout,
            "--http-timeout",
            "http-timeout",
            Self::duration,
        )?;
        p.with(
            &mut self.user_agent,
            "--user-agent",
            "user-agent",
            Self::cow,
        )?;
        p.with(&mut self.socks5, "--socks5", "socks5", |s| {
            Ok(Some(s.to_socket_addrs()?.collect()))
        })?;
        p.with(
            &mut self.socks5_restrict,
            "--socks5-restrict",
            "socks5-restrict",
            |h| Ok(Some(h.split(',').map(|h| h.trim().to_string()).collect())),
        )?;

        Ok(())
    }

    fn parse_options(&mut self, p: &mut Parser) -> Result<()> {
        p.opt(&mut self.client_id, "--client-id", "client-id")?;
        p.opt(&mut self.auth_token, "--auth-token", "auth-token")?;
        p.opt(
            &mut self.playlist_cache_dir,
            "--playlist-cache-dir",
            "playlist-cache-dir",
        )?;
        p.opt(&mut self.player_path, "-p", "player")?;
        p.opt(&mut self.record_path, "-r", "record")?;

        Ok(())
    }

    fn cow(s: &str) -> Result<Cow<'static, str>> {
        Ok(s.to_owned().into())
    }

    fn duration(s: &str) -> Result<Duration> {
        Ok(Duration::try_from_secs_f64(s.parse()?)?)
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
        self.0.remove(idx);

        if idx < self.0.len() {
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

struct Parser {
    args: Arguments,
    config: Option<String>,
}

impl Parser {
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
                let path = match args.take_value("-c") {
                    Some(p) => p,
                    None => Self::default_config_path()?,
                };

                if !args.contains("--no-config") && Path::new(&path).try_exists()? {
                    Some(fs::read_to_string(path).context("Failed to read config file")?)
                } else {
                    None
                }
            },
            args,
        })
    }

    fn with<T, F>(&mut self, dst: &mut T, key: &str, cfg_key: &str, f: F) -> Result<()>
    where
        F: Fn(&str) -> Result<T>,
    {
        if let Some(val) = self.raw_value(key, cfg_key)? {
            *dst = f(&val)?;
        }

        Ok(())
    }

    fn switch(&mut self, dst: &mut bool, key1: &str, key2: &str) -> Result<()> {
        let cfg = key2.trim_start_matches('-');
        if self.args.contains(key1) | self.args.contains(key2) {
            *dst = true;
        } else if let Some(val) = self.config_value(cfg) {
            *dst = val
                .parse()
                .with_context(|| format!("Invalid value for {cfg}"))?;
        }

        Ok(())
    }

    fn opt<T>(&mut self, dst: &mut Option<T>, key: &str, cfg_key: &str) -> Result<()>
    where
        T: FromStr,
        <T as FromStr>::Err: Display + Send + Sync + Error + 'static,
    {
        self.with(dst, key, cfg_key, |v| {
            Ok(Some(
                v.parse()
                    .with_context(|| format!("Invalid value for {cfg_key}"))?,
            ))
        })
    }

    fn free(&mut self) -> Option<String> {
        self.args.take_free()
    }

    fn remaining(self) -> Option<String> {
        self.args.0.into_iter().next()
    }

    fn raw_value(&mut self, key: &str, cfg_key: &str) -> Result<Option<String>> {
        if !key.is_empty() && self.args.0.iter().any(|k| k == key) {
            return Ok(Some(
                self.args
                    .take_value(key)
                    .with_context(|| format!("Missing value for {key}"))?,
            ));
        }

        Ok(self.config_value(cfg_key))
    }

    fn config_value(&self, key: &str) -> Option<String> {
        self.config.as_ref().and_then(|c| {
            c.lines().find_map(|l| {
                l.strip_prefix(key)
                    .filter(|r| r.starts_with('='))
                    .map(|r| r[1..].trim().to_string())
            })
        })
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    fn default_config_path() -> Result<String> {
        let dir = env::var("XDG_CONFIG_HOME")
            .unwrap_or_else(|_| format!("{}/.config", env::var("HOME").unwrap_or_default()));

        Ok(format!("{dir}/{}", constants::DEFAULT_CONFIG_PATH))
    }

    #[cfg(target_os = "windows")]
    fn default_config_path() -> Result<String> {
        Ok(format!(
            "{}/{}",
            env::var("APPDATA").unwrap_or_default(),
            constants::DEFAULT_CONFIG_PATH,
        ))
    }

    #[cfg(target_os = "macos")]
    fn default_config_path() -> Result<String> {
        Ok(format!(
            "{}/Library/Application Support/{}",
            env::var("HOME").unwrap_or_default(),
            constants::DEFAULT_CONFIG_PATH,
        ))
    }

    #[cfg(not(any(unix, target_os = "windows", target_os = "macos")))]
    fn default_config_path() -> Result<String> {
        Ok(constants::DEFAULT_CONFIG_PATH.to_string())
    }
}
