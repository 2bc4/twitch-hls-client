#![allow(clippy::unnecessary_wraps)] //function pointers

pub mod playlist;
pub mod segment;

use anyhow::{Context, Result};
use std::fmt::{self, Display, Formatter};

use crate::{
    args::{ArgParser, Parser},
    http::Url,
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
    client_id: Option<String>,
    auth_token: Option<String>,
    never_proxy: Option<Vec<String>>,
    codecs: String,
    no_low_latency: bool,
    channel: String,
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
        }
    }
}

impl ArgParser for Args {
    fn parse(&mut self, parser: &mut Parser) -> Result<()> {
        parser.parse_fn_cfg(&mut self.servers, "-s", "servers", Self::split_comma)?;
        parser.parse_fn(&mut self.client_id, "--client-id", Parser::parse_opt_string)?;
        parser.parse_fn(
            &mut self.auth_token,
            "--auth-token",
            Parser::parse_opt_string,
        )?;
        parser.parse(&mut self.codecs, "--codecs")?;
        parser.parse_fn(&mut self.never_proxy, "--never-proxy", Self::split_comma)?;
        parser.parse_switch(&mut self.no_low_latency, "--no-low-latency")?;

        self.channel = parser
            .parse_free_required::<String>()
            .context("Missing channel argument")?
            .to_lowercase()
            .replace("twitch.tv/", "");

        if let Some(ref never_proxy) = self.never_proxy {
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
}
