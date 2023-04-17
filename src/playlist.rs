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

use anyhow::{anyhow, Context, Result};
use log::{error, info};
use percent_encoding::{percent_encode, AsciiSet, CONTROLS};
use ureq::Agent;
use url::Url;

pub struct Segment {
    pub url: String,
    pub sequence: u64,
}

impl Segment {
    pub fn new(playlist: &str) -> Result<Self> {
        Ok(Self {
            url: playlist
                .lines()
                .last()
                .context("Malformed media playlist")?
                .replace("#EXT-X-TWITCH-PREFETCH:", ""),
            sequence: Self::parse_sequence(playlist)?,
        })
    }

    fn parse_sequence(playlist: &str) -> Result<u64> {
        playlist
            .lines()
            .skip_while(|s| !s.contains("#EXT-X-MEDIA-SEQUENCE"))
            .nth(1)
            .context("Malformed media playlist")?
            .split(':')
            .nth(1)
            .context("Invalid media sequence")?
            .parse()
            .context("Error parsing media sequence")
    }
}

pub struct PlaylistReload {
    pub segment: Segment,
    pub ad: bool,
    pub discontinuity: bool,
}

pub struct MediaPlaylist {
    agent: Agent,
    url: String,
}

impl MediaPlaylist {
    pub fn new(agent: &Agent, server: &str, channel: &str, quality: &str) -> Result<Self> {
        //TODO: Store for fallback servers on ad
        let master_playlist = MasterPlaylist::new(server, channel, quality)?;

        Ok(Self {
            agent: agent.clone(), //ureq uses an ARC to store state
            url: master_playlist.fetch(agent)?,
        })
    }

    pub fn catch_up(&self) -> Result<()> {
        info!("Catching up to latest segment");

        let mut segment = Segment::new(&self.fetch()?)?;
        let first = segment.sequence;
        while segment.sequence == first {
            segment = Segment::new(&self.fetch()?)?;
        }

        Ok(())
    }

    pub fn reload(&self) -> Result<PlaylistReload> {
        let playlist = self.fetch()?;
        Ok(PlaylistReload {
            segment: Segment::new(&playlist)?,
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
