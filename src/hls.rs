mod cache;
mod master_playlist;
mod media_playlist;
pub mod segment;

pub use master_playlist::fetch_playlist;
pub use media_playlist::MediaPlaylist;

use anyhow::{Context, Result};
use std::{
    borrow::Cow,
    fmt::{self, Display, Formatter},
};

use crate::{
    args::{Parse, Parser},
    http::{StatusError, Url},
};

#[derive(Debug)]
pub struct OfflineError;

impl std::error::Error for OfflineError {}

impl Display for OfflineError {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "Stream is offline or unavailable")
    }
}

#[derive(Debug)]
pub struct Args {
    servers: Option<Vec<Url>>,
    print_streams: bool,
    no_low_latency: bool,
    client_id: Option<String>,
    auth_token: Option<String>,
    codecs: Cow<'static, str>,
    never_proxy: Option<Vec<String>>,
    playlist_cache_dir: Option<String>,
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
            client_id: Option::default(),
            auth_token: Option::default(),
            never_proxy: Option::default(),
            playlist_cache_dir: Option::default(),
            force_playlist_url: Option::default(),
            channel: String::default(),
            quality: Option::default(),
        }
    }
}

impl Parse for Args {
    fn parse(&mut self, parser: &mut Parser) -> Result<()> {
        parser.parse_fn_cfg(&mut self.servers, "-s", "servers", Self::split_comma)?;
        parser.parse_switch(&mut self.print_streams, "--print-streams")?;
        parser.parse_switch(&mut self.no_low_latency, "--no-low-latency")?;
        parser.parse_opt_string(&mut self.client_id, "--client-id")?;
        parser.parse_opt_string(&mut self.auth_token, "--auth-token")?;
        parser.parse_cow_string(&mut self.codecs, "--codecs")?;
        parser.parse_fn(&mut self.never_proxy, "--never-proxy", Self::split_comma)?;
        parser.parse_opt_string(&mut self.playlist_cache_dir, "--playlist-cache-dir")?;
        parser.parse_fn(&mut self.force_playlist_url, "--force-playlist-url", |a| {
            Ok(Some(a.to_owned().into()))
        })?;

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
    #[allow(clippy::unnecessary_wraps, reason = "function pointer")]
    fn split_comma<T: for<'a> From<&'a str>>(arg: &str) -> Result<Option<Vec<T>>> {
        Ok(Some(arg.split(',').map(T::from).collect()))
    }
}

fn map_if_offline(error: anyhow::Error) -> anyhow::Error {
    if StatusError::is_not_found(&error) {
        return OfflineError.into();
    }

    error
}
