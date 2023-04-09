//    Copyright (C) 2023 2bc4
//
//    This program is free software: you can redistribute it and/or modify
//    it under the terms of the GNU General Public License as published by
//    the Free Software Foundation, either version 3 of the License, or
//    (at your option) any later version.
//
//    This program is distributed in the hope that it will be useful,
//    but WITHOUT ANY WARRANTY; without even the implied warranty of
//    MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
//    GNU General Public License for more details.
//
//    You should have received a copy of the GNU General Public License
//    along with this program.  If not, see <https://www.gnu.org/licenses/>.

use std::fmt;

use anyhow::{anyhow, bail, Context, Result};
use log::{error, info};
use percent_encoding::{percent_encode, AsciiSet, CONTROLS};
use ureq::{Agent, AgentBuilder, Error};
use url::Url;

use crate::USER_AGENT;

#[derive(Debug)]
pub enum PlaylistError {
    NotFoundError,
    DiscontinuityError,
}

impl std::error::Error for PlaylistError {}

impl fmt::Display for PlaylistError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::NotFoundError => write!(f, "Media playlist not found"),
            Self::DiscontinuityError => {
                write!(f, "Discontinuity encountered in media playlist")
            }
        }
    }
}

pub struct Segment {
    pub url: String,
    pub media_sequence: u64,
    pub has_discontinuity: bool,
}

impl Segment {
    pub fn new(playlist: &str) -> Result<Self> {
        Ok(Self {
            url: playlist
                .lines()
                .last()
                .context("Malformed media playlist")?
                .replace("#EXT-X-TWITCH-PREFETCH:", ""),
            media_sequence: Self::parse_media_sequence(playlist)?,
            has_discontinuity: playlist.contains("#EXT-X-DISCONTINUITY"),
        })
    }

    fn parse_media_sequence(playlist: &str) -> Result<u64> {
        playlist
            .lines()
            .skip_while(|s| !s.contains("#EXT-X-MEDIA-SEQUENCE:"))
            .nth(1)
            .context("Malformed media playlist")?
            .split(':')
            .nth(1)
            .context("Invalid media sequence")?
            .parse()
            .context("Error parsing media sequence")
    }
}

pub struct MediaPlaylist {
    agent: Agent,
    url: String,
}

impl MediaPlaylist {
    pub fn new(server: &str, channel: &str, quality: &str) -> Result<Self> {
        //TODO: Store for fallback servers on discontinuity
        let master_playlist = MasterPlaylist::new(server, channel, quality)?;

        Ok(Self {
            agent: AgentBuilder::new().user_agent(USER_AGENT).build(),
            url: master_playlist.fetch()?,
        })
    }

    pub fn catch_up(&self) -> Result<Segment> {
        info!("Catching up to latest segment");

        let mut segment = Segment::new(&self.fetch()?)?;
        let first = segment.media_sequence;
        while segment.media_sequence == first {
            segment = Segment::new(&self.fetch()?)?;
        }

        Ok(segment)
    }

    pub fn reload(&self) -> Result<Segment> {
        let segment = Segment::new(&self.fetch()?)?;
        if segment.has_discontinuity {
            return Err(PlaylistError::DiscontinuityError.into());
        }

        Ok(segment)
    }

    fn fetch(&self) -> Result<String> {
        match self
            .agent
            .get(&self.url)
            .set("referer", "https://player.twitch.tv")
            .set("origin", "https://player.twitch.tv")
            .set("connection", "keep-alive")
            .call()
        {
            Ok(response) => Ok(response.into_string()?),
            Err(Error::Status(code, _)) if code == 404 => Err(PlaylistError::NotFoundError.into()),
            Err(e) => bail!(e),
        }
    }
}

struct MasterPlaylist {
    agent: Agent, //only for user agent
    servers: Vec<Url>,
    quality: String,
    channel: String,
}

impl MasterPlaylist {
    pub fn new(servers: &str, channel: &str, quality: &str) -> Result<Self> {
        let channel = str::replace(channel, "twitch.tv/", "");

        let servers: Result<Vec<Url>, _> = servers
            .replace("[channel]", &channel)
            .split(',')
            .map(|s| {
                Url::parse_with_params(
                    s,
                    &[
                        ("player", "twitchweb"),
                        ("type", "any"),
                        ("allow_source", "true"),
                        ("allow_audio_only", "true"),
                        ("allow_spectre", "false"),
                        ("fast_bread", "true"),
                    ],
                )
            })
            .collect();

        Ok(Self {
            agent: AgentBuilder::new().user_agent(USER_AGENT).build(),
            servers: servers.context("Invalid server URL")?,
            quality: quality.to_owned(),
            channel,
        })
    }

    pub fn fetch(&self) -> Result<String> {
        info!("Fetching playlist for channel {}", self.channel);
        let playlist = self
            .servers
            .iter()
            .find_map(|s| {
                let request = if s.path() == "/[ttvlol]" {
                    info!("Trying TTVLOL API");
                    const ENCODE_SET: &AsciiSet = &CONTROLS.add(b'?').add(b'=').add(b'&');

                    let mut url = s.clone();
                    url.set_path(&("/playlist/".to_owned() + &self.channel + ".m3u8"));

                    self.agent
                        .get(&percent_encode(url.as_str().as_bytes(), ENCODE_SET).to_string())
                        .set("x-donate-to", "https://ttv.lol/donate")
                        .call()
                } else {
                    self.agent.request_url("GET", s).call()
                };

                match request {
                    Ok(res) => {
                        info!(
                            "Using server {}://{}",
                            s.scheme(),
                            s.host().expect("Somehow invalid host?")
                        );
                        res.into_string().ok()
                    }
                    Err(e) => {
                        error!("{}", e);
                        None
                    }
                }
            })
            .ok_or_else(|| anyhow!("No servers available"))?;

        Self::parse_variant_playlist(&self.quality, &playlist)
    }

    fn parse_variant_playlist(quality: &str, playlist: &str) -> Result<String> {
        Ok(playlist
            .lines()
            .skip_while(|s| {
                !(s.contains("#EXT-X-MEDIA") && (s.contains(quality) || quality == "best"))
            })
            .nth(2)
            .context("Invalid quality or malformed master playlist")?
            .to_owned())
    }
}
