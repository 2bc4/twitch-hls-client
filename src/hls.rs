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
    InvalidDuration,
    Advertisement,
    Discontinuity,
}

impl std::error::Error for Error {}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Unchanged => write!(f, "Media playlist is the same as previous"),
            Self::InvalidPrefetchUrl => write!(f, "Invalid or missing prefetch URLs"),
            Self::InvalidDuration => write!(f, "Invalid or missing segment duration"),
            Self::Advertisement => write!(f, "Encountered an embedded advertisement segment"),
            Self::Discontinuity => write!(f, "Encountered a discontinuity"),
        }
    }
}

#[derive(Copy, Clone)]
pub enum PrefetchUrlKind {
    Newest,
    Next,
}

//Option wrapper around Url because it doesn't implement Default
#[derive(Default, PartialEq, Eq)]
pub struct PrefetchUrls {
    newest: Option<Url>,
    next: Option<Url>,
}

impl PrefetchUrls {
    pub fn new(newest: &str, next: &str) -> Result<Self, Error> {
        Ok(Self {
            newest: Some(Url::parse(newest).or(Err(Error::InvalidPrefetchUrl))?),
            next: Some(Url::parse(next).or(Err(Error::InvalidPrefetchUrl))?),
        })
    }

    pub fn take(&mut self, kind: PrefetchUrlKind) -> Result<Url, Error> {
        match kind {
            PrefetchUrlKind::Newest => self.newest.take().ok_or(Error::InvalidPrefetchUrl),
            PrefetchUrlKind::Next => self.next.take().ok_or(Error::InvalidPrefetchUrl),
        }
    }
}

pub struct MediaPlaylist {
    pub urls: PrefetchUrls,
    pub duration: Duration,
    request: Request,
}

impl MediaPlaylist {
    pub fn new(url: &Url) -> Result<Self> {
        let mut request = Request::get(url.clone())?;
        request.set_accept_header(
            "application/x-mpegURL, application/vnd.apple.mpegurl, application/json, text/plain",
        )?;

        let mut media_playlist = Self {
            urls: PrefetchUrls::default(),
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

        let urls = Self::parse_prefetch_urls(&playlist)?;
        if urls == self.urls {
            return Err(Error::Unchanged.into());
        }

        self.urls = urls;
        self.duration = Self::parse_duration(&playlist)?;
        Ok(())
    }

    fn parse_prefetch_urls(playlist: &str) -> Result<PrefetchUrls, Error> {
        let newest = playlist
            .lines()
            .rev()
            .find(|s| s.starts_with("#EXT-X-TWITCH-PREFETCH"))
            .ok_or(Error::InvalidPrefetchUrl)?
            .split_once(':')
            .ok_or(Error::InvalidPrefetchUrl)?
            .1;

        let next = playlist
            .lines()
            .rev()
            .skip_while(|s| !s.starts_with("#EXT-X-TWITCH-PREFETCH"))
            .skip(1)
            .find(|s| s.starts_with("#EXT-X-TWITCH-PREFETCH"))
            .ok_or(Error::InvalidPrefetchUrl)?
            .split_once(':')
            .ok_or(Error::InvalidPrefetchUrl)?
            .1;

        PrefetchUrls::new(newest, next)
    }

    fn parse_duration(playlist: &str) -> Result<Duration, Error> {
        Duration::try_from_secs_f32(
            playlist
                .lines()
                .rev()
                .find(|s| s.starts_with("#EXTINF"))
                .ok_or(Error::InvalidDuration)?
                .split_once(':')
                .ok_or(Error::InvalidDuration)?
                .1
                .split_once(',')
                .ok_or(Error::InvalidDuration)?
                .0
                .parse()
                .or(Err(Error::InvalidDuration))?,
        )
        .or(Err(Error::InvalidDuration))
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

    pub fn fetch_variant_playlist(&self, channel: &str, quality: &str) -> Result<Url> {
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
                    Request::get(s.clone())
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
            .parse()?)
    }
}
