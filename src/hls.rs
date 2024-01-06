use std::{
    collections::hash_map::DefaultHasher,
    fmt,
    hash::Hasher,
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use log::{debug, error, info};
use rand::{
    distributions::{Alphanumeric, DistString},
    Rng,
};
use serde_json::{json, Value};
use url::Url;

use crate::{
    constants,
    http::{self, TextRequest},
};

#[derive(Debug)]
pub enum Error {
    Unchanged,
    InvalidPrefetchUrl,
    InvalidDuration,
    Offline,
    NotLowLatency(Url),
}

impl std::error::Error for Error {}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Unchanged => write!(f, "Media playlist is the same as previous"),
            Self::InvalidPrefetchUrl => write!(f, "Invalid or missing prefetch URLs"),
            Self::InvalidDuration => write!(f, "Invalid or missing segment duration"),
            Self::Offline => write!(f, "Stream is offline"),
            Self::NotLowLatency(_) => write!(f, "Stream is not low latency"),
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
    duration: Duration,
    request: TextRequest,
}

impl MediaPlaylist {
    pub fn new(url: &Url) -> Result<Self> {
        let mut playlist = Self {
            urls: PrefetchUrls::default(),
            duration: Duration::default(),
            request: TextRequest::get(url)?,
        };

        playlist.reload()?;
        Ok(playlist)
    }

    pub fn reload(&mut self) -> Result<()> {
        let mut playlist = self.fetch()?;

        let urls = PrefetchUrls::new(&playlist).or_else(|_| self.filter_ads(&mut playlist))?;
        if urls == self.urls {
            return Err(Error::Unchanged.into());
        }

        self.urls = urls;
        self.duration = Self::parse_duration(&playlist)?;

        Ok(())
    }

    pub fn sleep_segment_duration(&self, elapsed: Duration) {
        Self::sleep_thread(self.duration, elapsed);
    }

    pub fn sleep_half_segment_duration(&self, elapsed: Duration) {
        if let Some(half) = self.duration.checked_div(2) {
            Self::sleep_thread(half, elapsed);
        }
    }

    fn fetch(&mut self) -> Result<String> {
        let playlist = self.request.text().map_err(map_if_offline)?;
        debug!("Playlist:\n{playlist}");

        Ok(playlist)
    }

    fn filter_ads(&mut self, playlist: &mut String) -> Result<PrefetchUrls> {
        info!("Filtering ads...");
        loop {
            //Ads don't have prefetch URLs, wait until they come back to filter ads
            let time = Instant::now();
            *playlist = self.fetch()?;
            if let Ok(urls) = PrefetchUrls::new(playlist) {
                break Ok(urls);
            }

            self.duration = Self::parse_duration(playlist)?;
            self.sleep_segment_duration(time.elapsed());
        }
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

    fn sleep_thread(duration: Duration, elapsed: Duration) {
        if let Some(sleep_time) = duration.checked_sub(elapsed) {
            debug!("Sleeping thread for {:?}", sleep_time);
            thread::sleep(sleep_time);
        }
    }
}

pub fn fetch_proxy_playlist(servers: &[String], channel: &str, quality: &str) -> Result<Url> {
    info!("Fetching playlist for channel {} (proxy)", channel);
    let servers = servers
        .iter()
        .map(|s| {
            Url::parse_with_params(
                &s.replace("[channel]", channel),
                &[
                    ("allow_source", "true"),
                    ("allow_audio_only", "true"),
                    ("fast_bread", "true"),
                    ("warp", "true"),
                ],
            )
        })
        .collect::<Result<Vec<Url>, _>>()
        .context("Invalid server URL")?;

    let playlist = servers
        .iter()
        .find_map(|s| {
            info!("Using server {}://{}", s.scheme(), s.host_str().unwrap());
            let mut request = match TextRequest::get(s) {
                Ok(request) => request,
                Err(e) => {
                    error!("{e}");
                    return None;
                }
            };

            match request.text() {
                Ok(playlist_url) => Some(playlist_url),
                Err(e) => {
                    if matches!(e.downcast_ref::<http::Error>(), Some(http::Error::NotFound(_))) {
                        error!("Playlist not found. Stream offline?");
                        return None;
                    }

                    error!("{e}");
                    None
                }
            }
        })
        .ok_or(Error::Offline)?;

    parse_variant_playlist(&playlist, quality)
}

pub fn fetch_twitch_playlist(
    client_id: &Option<String>,
    auth_token: &Option<String>,
    channel: &str,
    quality: &str,
) -> Result<Url> {
    info!("Fetching playlist for channel {channel} (Twitch)");
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

    let mut request = TextRequest::post(&constants::TWITCH_GQL_ENDPOINT.parse()?, &gql.to_string())?;
    request.header("Content-Type: text/plain;charset=UTF-8")?;
    request.header(&format!("X-Device-ID: {}", &gen_id()))?;
    request.header(&format!(
        "Client-Id: {}",
        choose_client_id(client_id, auth_token)?
    ))?;

    if let Some(auth_token) = auth_token {
        request.header(&format!("Authorization: OAuth {auth_token}"))?;
    }

    let response: Value = serde_json::from_str(&request.text()?)?;
    let url = Url::parse_with_params(
        &format!("{}{channel}.m3u8", constants::TWITCH_HLS_BASE),
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
            ("warp", "true"),
        ],
    )?;

    parse_variant_playlist(&TextRequest::get(&url)?.text().map_err(map_if_offline)?, quality)
}

fn parse_variant_playlist(master_playlist: &str, quality: &str) -> Result<Url> {
    debug!("Master playlist:\n{master_playlist}");
    let playlist_url = master_playlist
        .lines()
        .skip_while(|s| !(s.contains("#EXT-X-MEDIA") && (s.contains(quality) || quality == "best")))
        .nth(2)
        .context("Invalid quality or malformed master playlist")?
        .parse::<Url>()?;

    if !master_playlist.contains("FUTURE=\"true\"") {
        return Err(Error::NotLowLatency(playlist_url).into());
    }

    Ok(playlist_url)
}

fn choose_client_id(client_id: &Option<String>, auth_token: &Option<String>) -> Result<String> {
    //--client-id > (if auth token) client id from twitch > default
    let client_id = if let Some(client_id) = client_id {
        client_id.clone()
    } else if let Some(auth_token) = auth_token {
        let mut request = TextRequest::get(&constants::TWITCH_OAUTH_ENDPOINT.parse()?)?;
        request.header(&format!("Authorization: OAuth {auth_token}"))?;

        let response: Value = serde_json::from_str(&request.text()?)?;

        //.to_string() adds quotes while .as_str() doesn't for some reason
        response["client_id"]
            .as_str()
            .context("Invalid client id in response")?
            .to_owned()
    } else {
        constants::DEFAULT_CLIENT_ID.to_owned()
    };

    Ok(client_id)
}

fn gen_id() -> String {
    Alphanumeric
        .sample_string(&mut rand::thread_rng(), 32)
        .to_lowercase()
}

fn map_if_offline(error: anyhow::Error) -> anyhow::Error {
    if matches!(
        error.downcast_ref::<http::Error>(),
        Some(http::Error::NotFound(_))
    ) {
        return Error::Offline.into();
    }

    error
}
