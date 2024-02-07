#![allow(clippy::unnecessary_wraps)] //function pointers

pub mod playlist;
pub mod segment;

use crate::args::{ArgParse, Parser};
use anyhow::{ensure, Context, Result};
use std::fmt;

#[derive(Debug)]
pub enum Error {
    Offline,
}

impl std::error::Error for Error {}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Offline => write!(f, "Stream is offline or unavailable"),
        }
    }
}

#[derive(Debug)]
pub struct Args {
    pub servers: Option<Vec<String>>,
    pub client_id: Option<String>,
    pub auth_token: Option<String>,
    pub never_proxy: Option<Vec<String>>,
    pub codecs: String,
    pub no_low_latency: bool,
    pub channel: String,
    pub quality: String,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            codecs: "av1,h265,h264".to_owned(),
            no_low_latency: bool::default(),
            servers: Option::default(),
            client_id: Option::default(),
            auth_token: Option::default(),
            never_proxy: Option::default(),
            channel: String::default(),
            quality: String::default(),
        }
    }
}

impl ArgParse for Args {
    fn parse(&mut self, parser: &mut Parser) -> Result<()> {
        parser.parse_fn_cfg(&mut self.servers, "-s", "servers", Self::split_comma)?;
        parser.parse_fn(&mut self.client_id, "--client-id", Self::parse_optstring)?;
        parser.parse_fn(&mut self.auth_token, "--auth-token", Self::parse_optstring)?;
        parser.parse(&mut self.codecs, "--codecs")?;
        parser.parse_fn(&mut self.never_proxy, "--never-proxy", Self::split_comma)?;
        parser.parse_switch(&mut self.no_low_latency, "--no-low-latency")?;

        self.channel = parser
            .parse_free_required::<String>()
            .context("Missing channel argument")?
            .to_lowercase()
            .replace("twitch.tv/", "");

        parser.parse_free(&mut self.quality, "quality")?;

        if let Some(ref never_proxy) = self.never_proxy {
            if never_proxy.iter().any(|a| a.eq(&self.channel)) {
                self.servers = None;
            }
        }

        ensure!(!self.quality.is_empty(), "Quality must be set");
        Ok(())
    }
}

impl Args {
    fn split_comma(arg: &str) -> Result<Option<Vec<String>>> {
        Ok(Some(arg.split(',').map(String::from).collect()))
    }

    fn parse_optstring(arg: &str) -> Result<Option<String>> {
        Ok(Some(arg.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    pub const PLAYLIST: &'static str = r#"#EXT3MU
#EXT-X-TARGETDURATION:6
#EXT-X-MEDIA-SEQUENCE:00000
#EXT-X-TWITCH-LIVE-SEQUENCE:00000
#EXT-X-TWITCH-ELAPSED-SECS:00000.000
#EXT-X-TWITCH-TOTAL-SECS:00000.000
#EXT-X-MAP:URI=http://header.invalid
#EXT-X-PROGRAM-DATE-TIME:1970-01-01T00:00:00.000Z
#EXTINF:5.020,live
http://segment1.invalid
#EXT-X-PROGRAM-DATE-TIME:1970-01-01T00:00:00.000Z
#EXTINF:4.910,live
http://segment2.invalid
#EXT-X-PROGRAM-DATE-TIME:1970-01-01T00:00:00.000Z
#EXTINF:2.002,Amazon
http://ad-segment.invalid
#EXT-X-PROGRAM-DATE-TIME:1970-01-01T00:00:00.000Z
#EXTINF:2.000,live
http://segment3.invalid
#EXT-X-PROGRAM-DATE-TIME:1970-01-01T00:00:00.000Z
#EXTINF:0.978,live
http://segment4.invalid
#EXT-X-TWITCH-PREFETCH:http://next-prefetch-url.invalid
#EXT-X-TWITCH-PREFETCH:http://newest-prefetch-url.invalid"#;
}
