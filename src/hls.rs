mod cache;
mod multivariant;
mod playlist;
mod segment;

pub use multivariant::Stream;
pub use playlist::Playlist;
pub use segment::{Handler, ResetError};

use std::{
    borrow::Cow,
    fmt::{self, Debug, Display, Formatter},
};

use anyhow::{Context, Result, bail, ensure};

use crate::{
    args::{Parse, Parser},
    http::{StatusError, Url},
};

#[derive(Debug)]
pub struct OfflineError;

impl std::error::Error for OfflineError {}

impl Display for OfflineError {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        f.write_str("Stream is offline or unavailable")
    }
}

pub struct Args {
    servers: Option<Vec<Url>>,
    print_streams: bool,
    no_low_latency: bool,
    passthrough: Passthrough,
    client_id: Option<String>,
    auth_token: Option<String>,
    codecs: Cow<'static, str>,
    never_proxy: Option<Vec<String>>,
    playlist_cache_dir: Option<String>,
    use_cache_only: bool,
    write_cache_only: bool,
    force_playlist_url: Option<Url>,
    channel: String,
    quality: Option<String>,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            codecs: "av1,h265,h264".into(),
            servers: Option::default(),
            print_streams: bool::default(),
            no_low_latency: bool::default(),
            passthrough: Passthrough::default(),
            client_id: Option::default(),
            auth_token: Option::default(),
            never_proxy: Option::default(),
            playlist_cache_dir: Option::default(),
            use_cache_only: bool::default(),
            write_cache_only: bool::default(),
            force_playlist_url: Option::default(),
            channel: String::default(),
            quality: Option::default(),
        }
    }
}

impl Debug for Args {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        f.debug_struct("Args")
            .field("servers", &self.servers)
            .field("print_streams", &self.print_streams)
            .field("no_low_latency", &self.no_low_latency)
            .field("passthrough", &self.passthrough)
            .field("client_id", &Self::hide_option(&self.client_id))
            .field("auth_token", &Self::hide_option(&self.auth_token))
            .field("codecs", &self.codecs)
            .field("never_proxy", &self.never_proxy)
            .field("playlist_cache_dir", &self.playlist_cache_dir)
            .field("use_cache_only", &self.use_cache_only)
            .field("write_cache_only", &self.write_cache_only)
            .field("force_playlist_url", &self.force_playlist_url)
            .field("channel", &self.channel)
            .field("quality", &self.quality)
            .finish()
    }
}

impl Parse for Args {
    fn parse(&mut self, parser: &mut Parser) -> Result<()> {
        parser.parse_fn_cfg(&mut self.servers, "-s", "servers", Self::split_comma)?;
        parser.parse_switch(&mut self.print_streams, "--print-streams")?;
        parser.parse_switch(&mut self.no_low_latency, "--no-low-latency")?;
        parser.parse_fn(&mut self.passthrough, "--passthrough", Passthrough::new)?;
        parser.parse_opt(&mut self.client_id, "--client-id")?;
        parser.parse_opt(&mut self.auth_token, "--auth-token")?;
        parser.parse_cow_string(&mut self.codecs, "--codecs")?;
        parser.parse_fn(&mut self.never_proxy, "--never-proxy", Self::split_comma)?;
        parser.parse_opt(&mut self.playlist_cache_dir, "--playlist-cache-dir")?;
        parser.parse_switch(&mut self.use_cache_only, "--use-cache-only")?;
        parser.parse_switch(&mut self.write_cache_only, "--write-cache-only")?;
        parser.parse_opt(&mut self.force_playlist_url, "--force-playlist-url")?;

        if self.use_cache_only || self.write_cache_only {
            ensure!(
                self.playlist_cache_dir.is_some(),
                "--playlist-cache-dir not configured"
            );
        }

        ensure!(
            !(self.use_cache_only && self.write_cache_only),
            "--use-cache-only and --write-cache-only cannot be used together"
        );

        let channel = parser
            .parse_free_required()
            .context("Missing channel argument")?;

        self.channel = channel
            .rsplit_once('/')
            .map_or(channel.as_str(), |s| s.1)
            .to_lowercase();

        parser.parse_free(&mut self.quality, "quality")?;
        if self.print_streams {
            self.quality = None;
        }

        if let Some(never_proxy) = &self.never_proxy {
            if never_proxy.iter().any(|a| a.eq(&self.channel)) {
                self.servers = None;
            }
        }

        Ok(())
    }
}

impl Args {
    fn split_comma<T: for<'a> From<&'a str>>(arg: &str) -> Result<Option<Vec<T>>> {
        Ok(Some(arg.split(',').map(T::from).collect()))
    }

    const fn hide_option(arg: &Option<String>) -> Option<&'static str> {
        match arg {
            Some(_) => Some("<hidden>"),
            None => None,
        }
    }
}

#[derive(Debug, Default)]
enum Passthrough {
    Variant,
    Multivariant,

    #[default]
    Disabled,
}

impl Passthrough {
    fn new(arg: &str) -> Result<Self> {
        match arg {
            "variant" => Ok(Self::Variant),
            "multivariant" => Ok(Self::Multivariant),
            "disabled" => Ok(Self::Disabled),
            _ => bail!("Invalid passthrough mode"),
        }
    }
}

fn map_if_offline(error: anyhow::Error) -> anyhow::Error {
    if StatusError::is_not_found(&error) {
        return OfflineError.into();
    }

    error
}
