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

use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use log::{error, info};
use percent_encoding::{percent_encode, AsciiSet, CONTROLS};
use ureq::Agent;
use url::Url;

pub struct Reload {
    pub url: String,
    pub sequence: u32,
    pub prev_sequence: u32,
    pub delta: Duration,

    pub ad: bool,
    pub discontinuity: bool,
}

pub struct MediaPlaylist {
    agent: Agent,
    url: String,
    prev_sequence: u32,
    prev_total_seconds: Duration,
}

impl MediaPlaylist {
    pub fn new(agent: &Agent, server: &str, channel: &str, quality: &str) -> Result<Self> {
        //TODO: Store for fallback servers on ad
        let master_playlist = MasterPlaylist::new(server, channel, quality)?;

        Ok(Self {
            agent: agent.clone(), //ureq uses an ARC to store state
            url: master_playlist.fetch(agent)?,
            prev_sequence: 0,
            prev_total_seconds: Duration::ZERO,
        })
    }

    pub fn catch_up(&mut self) -> Result<()> {
        info!("Catching up to latest segment");

        let mut playlist = self.fetch()?;
        let mut sequence = Self::parse_sequence(&playlist)?;

        let first = sequence;
        while sequence == first {
            playlist = self.fetch()?;
            sequence = Self::parse_sequence(&playlist)?;
        }

        self.prev_sequence = sequence;
        self.prev_total_seconds = Self::parse_total_seconds(&playlist)?;

        Ok(())
    }

    pub fn reload(&mut self) -> Result<Reload> {
        let playlist = self.fetch()?;

        let sequence = Self::parse_sequence(&playlist)?;
        let prev_sequence = self.prev_sequence;
        self.prev_sequence = sequence;

        let total_seconds = Self::parse_total_seconds(&playlist)?;
        let prev_total_seconds = self.prev_total_seconds;
        self.prev_total_seconds = total_seconds;

        Ok(Reload {
            url: Self::parse_url(&playlist)?,
            sequence,
            prev_sequence,
            delta: total_seconds - prev_total_seconds,
            ad: playlist.contains("Amazon")
                || playlist.contains("stitched-ad")
                || playlist.contains("X-TV-TWITCH-AD"),
            discontinuity: playlist.contains("#EXT-X-DISCONTINUITY"),
        })
    }

    fn fetch(&self) -> Result<String> {
        Ok(self
            .agent
            .get(&self.url)
            .set("referer", "https://player.twitch.tv")
            .set("origin", "https://player.twitch.tv")
            .set("connection", "keep-alive")
            .call()
            .context("Failed to fetch media playlist")?
            .into_string()?)
    }

    fn parse_sequence(playlist: &str) -> Result<u32> {
        playlist
            .lines()
            .skip_while(|s| !s.starts_with("#EXT-X-MEDIA-SEQUENCE"))
            .nth(1)
            .context("Malformed media playlist while parsing #EXT-X-MEDIA-SEQUENCE")?
            .split(':')
            .nth(1)
            .context("Invalid #EXT-X-MEDIA-SEQUENCE")?
            .parse()
            .context("Error parsing #EXT-X-MEDIA-SEQUENCE")
    }

    fn parse_url(playlist: &str) -> Result<String> {
        Ok(playlist
            .lines()
            .last()
            .context("Malformed media playlist while parsing segment URL")?
            .replace("#EXT-X-TWITCH-PREFETCH:", ""))
    }

    fn parse_total_seconds(playlist: &str) -> Result<Duration> {
        Ok(Duration::try_from_secs_f32(
            playlist
                .lines()
                .skip_while(|s| !s.starts_with("#EXT-X-TWITCH-TOTAL-SECS"))
                .next()
                .context("Malformed media playlist while parsing #EXT-X-TWITCH-TOTAL-SECS")?
                .split(':')
                .nth(1)
                .context("Invalid #EXT-X-TWITCH-TOTAL-SECS")?
                .parse()
                .context("Error parsing #EXT-X-TWITCH-TOTAL-SECS")?,
        )?)
    }
}

struct MasterPlaylist {
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
            servers: servers.context("Invalid server URL")?,
            quality: quality.to_owned(),
            channel,
        })
    }

    pub fn fetch(&self, agent: &Agent) -> Result<String> {
        info!("Fetching playlist for channel {}", self.channel);
        let playlist = self
            .servers
            .iter()
            .find_map(|s| {
                info!(
                    "Using server {}://{}",
                    s.scheme(),
                    s.host().expect("Somehow invalid host?")
                );

                let request = if s.path() == "/[ttvlol]" {
                    info!("Trying TTVLOL API");
                    const ENCODE_SET: &AsciiSet = &CONTROLS.add(b'?').add(b'=').add(b'&');

                    let mut url = s.clone();
                    url.set_path(&("/playlist/".to_owned() + &self.channel + ".m3u8"));

                    agent
                        .get(&percent_encode(url.as_str().as_bytes(), ENCODE_SET).to_string())
                        .set("x-donate-to", "https://ttv.lol/donate")
                        .call()
                } else {
                    agent.request_url("GET", s).call()
                };

                match request {
                    Ok(res) => res.into_string().ok(),
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
