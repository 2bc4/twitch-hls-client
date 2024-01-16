use std::{
    collections::hash_map::DefaultHasher,
    fmt,
    hash::Hasher,
    str::FromStr,
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
            Self::Offline => write!(f, "Stream is offline or unavailable"),
            Self::NotLowLatency(_) => write!(f, "Stream is not low latency"),
        }
    }
}

//Used for av1/hevc streams
#[derive(Default)]
pub struct SegmentHeaderUrl(pub Option<Url>);

impl FromStr for SegmentHeaderUrl {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let header_url = s.lines().find_map(|s| {
            s.starts_with("#EXT-X-MAP")
                .then_some(s)
                .and_then(|s| s.split_once('='))
                .map(|s| s.1.replace('"', ""))
        });

        if let Some(header_url) = header_url {
            return Ok(Self(Some(header_url.parse()?)));
        }

        Ok(Self(None))
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

impl FromStr for PrefetchUrls {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut hasher = DefaultHasher::new();
        let mut iter = s.lines().rev().filter_map(|s| {
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
}

impl PartialEq for PrefetchUrls {
    fn eq(&self, other: &Self) -> bool {
        self.hash == other.hash
    }
}

impl PrefetchUrls {
    pub fn take(&mut self, kind: PrefetchUrlKind) -> Result<Url, Error> {
        match kind {
            PrefetchUrlKind::Newest => self.newest.take().ok_or(Error::InvalidPrefetchUrl),
            PrefetchUrlKind::Next => self.next.take().ok_or(Error::InvalidPrefetchUrl),
        }
    }
}

#[derive(Copy, Clone)]
pub enum SleepLength {
    Full,
    Half,
}

#[derive(Default)]
pub struct SegmentDuration(Duration);

impl FromStr for SegmentDuration {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(
            Duration::try_from_secs_f32(
                s.lines()
                    .rev()
                    .find(|s| s.starts_with("#EXTINF"))
                    .and_then(|s| s.split_once(':'))
                    .and_then(|s| s.1.split_once(','))
                    .map(|s| s.0)
                    .ok_or(Error::InvalidDuration)?
                    .parse()
                    .or(Err(Error::InvalidDuration))?,
            )
            .or(Err(Error::InvalidDuration))?,
        ))
    }
}

impl SegmentDuration {
    pub fn sleep(&self, length: SleepLength, elapsed: Duration) {
        match length {
            SleepLength::Full => Self::sleep_thread(self.0, elapsed),
            SleepLength::Half => {
                if let Some(half) = self.0.checked_div(2) {
                    Self::sleep_thread(half, elapsed);
                }
            }
        }
    }

    fn sleep_thread(duration: Duration, elapsed: Duration) {
        if let Some(sleep_time) = duration.checked_sub(elapsed) {
            debug!("Sleeping thread for {:?}", sleep_time);
            thread::sleep(sleep_time);
        }
    }
}

pub struct MediaPlaylist {
    pub header_url: SegmentHeaderUrl,
    pub urls: PrefetchUrls,
    pub duration: SegmentDuration,
    request: TextRequest,
}

impl MediaPlaylist {
    pub fn new(url: &Url) -> Result<Self> {
        let mut playlist = Self {
            header_url: SegmentHeaderUrl::default(),
            urls: PrefetchUrls::default(),
            duration: SegmentDuration::default(),
            request: TextRequest::get(url)?,
        };

        playlist.header_url = playlist.reload()?.parse()?;
        Ok(playlist)
    }

    pub fn reload(&mut self) -> Result<String> {
        let mut playlist = self.fetch()?;

        let urls = playlist
            .parse()
            .or_else(|_| self.filter_ads(&mut playlist))?;

        if urls == self.urls {
            return Err(Error::Unchanged.into());
        }

        self.urls = urls;
        self.duration = playlist.parse()?;
        Ok(playlist)
    }

    fn fetch(&mut self) -> Result<String> {
        let playlist = self.request.text().map_err(map_if_offline)?;
        debug!("Playlist:\n{playlist}");

        Ok(playlist)
    }

    fn filter_ads(&mut self, playlist: &mut String) -> Result<PrefetchUrls> {
        info!("Filtering ads...");
        //Ads don't have prefetch URLs, wait until they come back to filter ads
        loop {
            let time = Instant::now();
            *playlist = self.fetch()?;

            if let Ok(urls) = playlist.parse() {
                break Ok(urls);
            }

            self.duration = playlist.parse()?;
            self.duration.sleep(SleepLength::Full, time.elapsed());
        }
    }
}

pub fn fetch_proxy_playlist(
    servers: &[String],
    codecs: &str,
    channel: &str,
    quality: &str,
) -> Result<Url> {
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
                    ("supported_codecs", codecs),
                ],
            )
        })
        .collect::<Result<Vec<Url>, _>>()
        .context("Invalid server URL")?;

    let playlist = servers
        .iter()
        .find_map(|s| {
            info!(
                "Using server {}://{}",
                s.scheme(),
                s.host_str().unwrap_or("<unknown>")
            );

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
                    if matches!(
                        e.downcast_ref::<http::Error>(),
                        Some(http::Error::NotFound(_))
                    ) {
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
    codecs: &str,
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
    })
    .to_string();

    let mut request = TextRequest::post(&constants::TWITCH_GQL_ENDPOINT.parse()?, &gql)?;
    request.header("Content-Type: text/plain;charset=UTF-8")?;
    request.header(&format!("X-Device-ID: {}", &gen_id()))?;
    request.header(&format!(
        "Client-Id: {}",
        choose_client_id(client_id, auth_token)?
    ))?;

    if let Some(auth_token) = auth_token {
        request.header(&format!("Authorization: OAuth {auth_token}"))?;
    }

    let response = request.text()?;
    debug!("GQL response: {response}");

    let response = serde_json::from_str::<Value>(&response).context("Invalid GQL response")?;
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
            ("supported_codecs", codecs),
            ("transcode_mode", "cbr_v1"),
            (
                "p",
                &rand::thread_rng().gen_range(0..=9_999_999).to_string(),
            ),
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

    parse_variant_playlist(
        &TextRequest::get(&url)?.text().map_err(map_if_offline)?,
        quality,
    )
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

        let response = serde_json::from_str::<Value>(&request.text()?)?;
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
