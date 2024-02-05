#![allow(clippy::unnecessary_wraps)] //function pointers

pub mod handler;
pub mod playlist;
pub mod segment;

use crate::args::{ArgParse, Parser};
use anyhow::{ensure, Context, Result};
use std::fmt;

#[derive(Debug)]
pub enum Error {
    Offline,
    Advertisement,
}

impl std::error::Error for Error {}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Offline => write!(f, "Stream is offline or unavailable"),
            Self::Advertisement => write!(f, "Encountered an embedded advertisement"),
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
    pub low_latency: bool,
    pub channel: String,
    pub quality: String,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            codecs: "av1,h265,h264".to_owned(),
            low_latency: true,
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

        let mut no_low_latency = bool::default();
        parser.parse_switch(&mut no_low_latency, "--no-low-latency")?;
        self.low_latency = !no_low_latency;

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

/* TODO: Re-add tests */
