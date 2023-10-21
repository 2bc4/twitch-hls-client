use std::{collections::hash_map::DefaultHasher, fmt, hash::Hasher, time::Duration};

use anyhow::{anyhow, Context, Result};
use log::{debug, error, info};
use rand::{
    distributions::{Alphanumeric, DistString},
    Rng,
};
use serde_json::{json, Value};
use url::Url;

use crate::http::Request;

#[derive(Debug)]
pub enum Error {
    Unchanged,
    InvalidPrefetchUrl,
    InvalidDuration,
    NotLowLatency(String),
}

impl std::error::Error for Error {}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Unchanged => write!(f, "Media playlist is the same as previous"),
            Self::InvalidPrefetchUrl => write!(f, "Invalid or missing prefetch URLs"),
            Self::InvalidDuration => write!(f, "Invalid or missing segment duration"),
            Self::NotLowLatency(_) => write!(f, "Stream is not low latency or is missing prefetch URLs"),
        }
    }
}

#[derive(Copy, Clone)]
pub enum PrefetchUrlKind {
    Newest,
    Next,
}

//Option wrapper around Url because it doesn't implement Default
#[derive(Default)]
pub struct PrefetchUrls {
    newest: Option<Url>,
    next: Option<Url>,
    hash: u64,
}

impl PartialEq for PrefetchUrls {
    fn eq(&self, other: &Self) -> bool {
        self.hash == other.hash
    }
}

impl PrefetchUrls {
    pub fn new(playlist: &str) -> Result<Self, Error> {
        let mut hasher = DefaultHasher::new();
        let mut iter = playlist.lines().rev().filter_map(|s| {
            s.starts_with("#EXT-X-TWITCH-PREFETCH")
                .then_some(s)
                .and_then(|s| s.split_once(':'))
                .and_then(|s| {
                    hasher.write(s.1.as_bytes());
                    Url::parse(s.1).ok()
                })
        });

        Ok(Self {
            newest: Some(iter.next().ok_or(Error::InvalidPrefetchUrl)?),
            next: Some(iter.next().ok_or(Error::InvalidPrefetchUrl)?),
            hash: hasher.finish(),
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
    pub fn new(url: Url) -> Result<Self> {
        let mut playlist = Self {
            urls: PrefetchUrls::default(),
            duration: Duration::default(),
            request: Request::get(url)?,
        };

        playlist.reload()?;
        Ok(playlist)
    }

    pub fn reload(&mut self) -> Result<()> {
        let playlist = self.request.read_string()?;
        debug!("Playlist:\n{playlist}");

        let urls = PrefetchUrls::new(&playlist)?;
        if urls == self.urls {
            return Err(Error::Unchanged.into());
        }

        self.urls = urls;
        self.duration = Self::parse_duration(&playlist)?;

        Ok(())
    }

    fn parse_duration(playlist: &str) -> Result<Duration, Error> {
        Duration::try_from_secs_f32(
            playlist
                .lines()
                .rev()
                .find(|s| s.starts_with("#EXTINF"))
                .and_then(|s| s.split_once(':'))
                .and_then(|s| s.1.split_once(','))
                .map(|s| s.0)
                .ok_or(Error::InvalidDuration)?
                .parse()
                .or(Err(Error::InvalidDuration))?,
        )
        .or(Err(Error::InvalidDuration))
    }
}

pub fn fetch_proxy_playlist(servers: &[String], channel: &str, quality: &str) -> Result<Url> {
    info!("Fetching playlist for channel {} (proxy)", channel);
    let servers = servers
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
        .context("Invalid server URL")?;

    let (request, playlist) = servers
        .iter()
        .find_map(|s| {
            info!("Using server {}://{}", s.scheme(), s.host_str().unwrap());
            let mut request = match Request::get(s.clone()) {
                Ok(request) => request,
                Err(e) => {
                    error!("{e}");
                    return None;
                }
            };

            match request.read_string() {
                Ok(playlist_url) => Some((request, playlist_url)),
                Err(e) => {
                    error!("{e}");
                    None
                }
            }
        })
        .ok_or_else(|| anyhow!("No servers available"))?;

    parse_variant_playlist(request.url_string(), &playlist, quality)
}

pub fn fetch_twitch_playlist(
    client_id: &Option<String>,
    auth_token: &Option<String>,
    channel: &str,
    quality: &str,
) -> Result<Url> {
    info!("Fetching playlist for channel {} (Twitch)", channel);
    let gql = json!({
        "operationName": "PlaybackAccessToken",
        "extensions": {
            "persistedQuery": {
                "version": 1,
                "sha256Hash": "0828119ded1c13477966434e15800ff57ddacf13ba1911c129dc2200705b0712",
            },
        },
        "variables": {
            "isLive": true,
            "login": channel,
            "isVod": false,
            "vodID": "",
            "playerType": "site",
        },
    });

    let mut request = Request::post_gql(&gen_id(), client_id, auth_token, &gql.to_string())?;
    let response: Value = serde_json::from_str(&request.read_string()?)?;

    let url = Url::parse_with_params(
        &format!("https://usher.ttvnw.net/api/channel/hls/{channel}.m3u8"),
        &[
            ("acmb", "e30="),
            ("allow_source", "true"),
            ("allow_audio_only", "true"),
            ("cdm", "wv"),
            ("fast_bread", "true"),
            ("playlist_include_framerate", "true"),
            ("player_backend", "mediaplayer"),
            ("reassignments_supported", "true"),
            ("supported_codecs", "avc1"),
            ("transcode_mode", "cbr_v1"),
            ("p", &rand::thread_rng().gen_range(0..=9_999_999).to_string()),
            ("play_session_id", &gen_id()),
            (
                "sig",
                response["data"]["streamPlaybackAccessToken"]["signature"]
                    .as_str()
                    .context("Invalid signature")?,
            ),
            (
                "token",
                response["data"]["streamPlaybackAccessToken"]["value"]
                    .as_str()
                    .context("Invalid token")?,
            ),
            ("player_version", "1.23.0"),
        ],
    )?;
    request = Request::get(url)?;

    parse_variant_playlist(request.url_string(), &request.read_string()?, quality)
}

fn parse_variant_playlist(url: String, playlist: &str, quality: &str) -> Result<Url> {
    debug!("Master playlist:\n{playlist}");
    if !playlist.contains(r#"FUTURE="true""#) {
        return Err(Error::NotLowLatency(url).into());
    }

    Ok(playlist
        .lines()
        .skip_while(|s| !(s.contains("#EXT-X-MEDIA") && (s.contains(quality) || quality == "best")))
        .nth(2)
        .context("Invalid quality or malformed master playlist")?
        .parse()?)
}

fn gen_id() -> String {
    Alphanumeric
        .sample_string(&mut rand::thread_rng(), 32)
        .to_lowercase()
}
