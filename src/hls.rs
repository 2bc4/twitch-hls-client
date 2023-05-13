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

use std::{fmt, time::Duration};

use anyhow::{anyhow, Context, Result};
use log::{debug, error, info};
use url::Url;

use crate::http::Request;

#[derive(Debug)]
pub enum Error {
    Unchanged,
    InvalidPrefetchUrl,
    Advertisement,
    Discontinuity,
}

impl std::error::Error for Error {}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Unchanged => write!(f, "Media playlist is the same as previous"),
            Self::InvalidPrefetchUrl => write!(f, "Invalid or missing prefetch URLs"),
            Self::Advertisement => write!(f, "Encountered an embedded advertisement segment"),
            Self::Discontinuity => write!(f, "Encountered a discontinuity"),
        }
    }
}

pub struct MediaPlaylist {
    pub prefetch_urls: Vec<String>, //[0] = newest, [1] = next
    pub duration: Duration,
    request: Request,
}

impl MediaPlaylist {
    pub fn new(playlist_url: &str) -> Result<Self> {
        let mut request = Request::get(playlist_url)?;
        request.set_accept_header(
            "application/x-mpegURL, application/vnd.apple.mpegurl, application/json, text/plain",
        )?;

        let mut media_playlist = Self {
            prefetch_urls: vec![String::default(); 2],
            duration: Duration::default(),
            request,
        };

        media_playlist.reload()?;
        Ok(media_playlist)
    }

    pub fn reload(&mut self) -> Result<()> {
        let playlist = self.request.read_string()?;
        debug!("Playlist:\n{playlist}");

        if playlist.contains("Amazon")
            || playlist.contains("stitched-ad")
            || playlist.contains("X-TV-TWITCH-AD")
        {
            return Err(Error::Advertisement.into());
        }

        if playlist.contains("#EXT-X-DISCONTINUITY") {
            return Err(Error::Discontinuity.into());
        }

        let prefetch_urls = Self::parse_prefetch_urls(&playlist)?;
        if prefetch_urls[0] == self.prefetch_urls[0] || prefetch_urls[1] == self.prefetch_urls[1] {
            return Err(Error::Unchanged.into());
        }

        self.prefetch_urls = prefetch_urls;
        self.duration = Self::parse_duration(&playlist)?;
        Ok(())
    }

    fn parse_prefetch_urls(playlist: &str) -> Result<Vec<String>> {
        let prefetch_urls = playlist
            .lines()
            .rev()
            .filter(|s| s.starts_with("#EXT-X-TWITCH-PREFETCH"))
            .map(|s| s.replace("#EXT-X-TWITCH-PREFETCH:", ""))
            .collect::<Vec<String>>();

        if prefetch_urls.len() != 2 {
            return Err(Error::InvalidPrefetchUrl.into());
        }

        for url in &prefetch_urls {
            Url::parse(url).or(Err(Error::InvalidPrefetchUrl))?;
        }

        Ok(prefetch_urls)
    }

    fn parse_duration(playlist: &str) -> Result<Duration> {
        Ok(Duration::try_from_secs_f32(
            playlist
                .lines()
                .rev()
                .find(|s| s.starts_with("#EXTINF"))
                .context("Malformed media playlist while parsing #EXTINF")?
                .replace("#EXTINF:", "")
                .split(',')
                .next()
                .context("Invalid #EXTINF")?
                .parse()
                .context("Failed to parse #EXTINF")?,
        )?)
    }
}

pub struct MasterPlaylist {
    servers: Vec<Url>,
}

impl MasterPlaylist {
    pub fn new(servers: &[String]) -> Result<Self> {
        Ok(Self {
            servers: servers
                .iter()
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
                .collect::<Result<Vec<Url>, _>>()
                .context("Invalid server URL")?,
        })
    }

    pub fn fetch(&self, channel: &str, quality: &str) -> Result<String> {
        info!("Fetching playlist for channel {}", channel);
        let playlist = self
            .servers
            .iter()
            .find_map(|s| {
                let scheme = s.scheme();
                let host = s.host_str().expect("Somehow invalid host?");

                let request = if s.path() == "/[ttvlol]" {
                    info!("Using server {scheme}://{host} (TTVLOL API)");

                    let mut url = s.clone();
                    url.set_path(&format!("/playlist/{channel}.m3u8"));

                    Request::get_with_header(
                        &url.as_str()
                            .replace('?', "%3F")
                            .replace('=', "%3D")
                            .replace('&', "%26"),
                        "X-Donate-To: https://ttv.lol/donate",
                    )
                } else {
                    info!("Using server {scheme}://{host}");
                    Request::get(s.as_str())
                };

                //Awkward but I do just want to print the error and move on
                match request {
                    Ok(mut res) => match res.read_string() {
                        Ok(res) => Some(res),
                        Err(e) => {
                            error!("{e}");
                            None
                        }
                    },
                    Err(e) => {
                        error!("{e}");
                        None
                    }
                }
            })
            .ok_or_else(|| anyhow!("No servers available"))?;

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
