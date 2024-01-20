use std::{
    collections::hash_map::DefaultHasher,
    fmt,
    hash::Hasher,
    iter,
    str::FromStr,
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use log::{debug, error, info};
use url::Url;

use crate::{
    args::HlsArgs,
    constants,
    http::{self, Agent, TextRequest},
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
    pub fn take_newest(&mut self) -> Result<Url, Error> {
        self.newest.take().ok_or(Error::InvalidPrefetchUrl)
    }

    pub fn take_next(&mut self) -> Result<Url, Error> {
        self.next.take().ok_or(Error::InvalidPrefetchUrl)
    }
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
    pub fn sleep(&self, elapsed: Duration) {
        Self::sleep_thread(self.0, elapsed);
    }

    pub fn sleep_half(&self, elapsed: Duration) {
        if let Some(half) = self.0.checked_div(2) {
            Self::sleep_thread(half, elapsed);
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
    pub fn new(url: &Url, agent: &Agent) -> Result<Self> {
        let mut playlist = Self {
            header_url: SegmentHeaderUrl::default(),
            urls: PrefetchUrls::default(),
            duration: SegmentDuration::default(),
            request: agent.get(url)?,
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
            self.duration.sleep(time.elapsed());
        }
    }
}

struct PlaybackAccessToken {
    token: String,
    signature: String,
    play_session_id: String,
}

impl PlaybackAccessToken {
    fn new(
        client_id: &Option<String>,
        auth_token: &Option<String>,
        channel: &str,
        agent: &Agent,
    ) -> Result<Self> {
        #[rustfmt::skip]
        let gql = concat!(
        "{",
            "\"extensions\":{",
                "\"persistedQuery\":{",
                    r#""sha256Hash":"0828119ded1c13477966434e15800ff57ddacf13ba1911c129dc2200705b0712","#,
                    "\"version\":1",
                "}",
            "},",
            r#""operationName":"PlaybackAccessToken","#,
            "\"variables\":{",
                "\"isLive\":true,",
                "\"isVod\":false,",
                r#""login":"{channel}","#,
                r#""playerType": "site","#,
                r#""vodID":"""#,
            "}",
        "}").replace("{channel}", channel);

        let mut request = agent.post(&constants::TWITCH_GQL_ENDPOINT.parse()?, &gql)?;
        request.header("Content-Type: text/plain;charset=UTF-8")?;
        request.header(&format!("X-Device-ID: {}", &Self::gen_id()))?;
        request.header(&format!(
            "Client-Id: {}",
            Self::choose_client_id(client_id, auth_token, agent)?
        ))?;

        if let Some(auth_token) = auth_token {
            request.header(&format!("Authorization: OAuth {auth_token}"))?;
        }

        let response = request.text()?;
        debug!("GQL response: {response}");

        Ok(Self {
            token: {
                let start = response
                    .find(r#"{\"adblock\""#)
                    .context("Failed to parse token start in GQL response")?;

                let end = response
                    .find(r#"","signature""#)
                    .context("Failed to parse token end in GQL response")?;

                response[start..end].replace('\\', "")
            },
            signature: response
                .split_once(r#""signature":""#)
                .context("Failed to parse signature in GQL response")?
                .1
                .chars()
                .take(40)
                .collect::<String>(),
            play_session_id: Self::gen_id(),
        })
    }

    fn choose_client_id(
        client_id: &Option<String>,
        auth_token: &Option<String>,
        agent: &Agent,
    ) -> Result<String> {
        //--client-id > (if auth token) client id from twitch > default
        let client_id = if let Some(client_id) = client_id {
            client_id.to_owned()
        } else if let Some(auth_token) = auth_token {
            let mut request = agent.get(&constants::TWITCH_OAUTH_ENDPOINT.parse()?)?;
            request.header(&format!("Authorization: OAuth {auth_token}"))?;

            request
                .text()?
                .split_once(r#""client_id":""#)
                .context("Failed to parse client id in GQL response")?
                .1
                .chars()
                .take(30)
                .collect::<String>()
        } else {
            constants::DEFAULT_CLIENT_ID.to_owned()
        };

        Ok(client_id)
    }

    fn gen_id() -> String {
        iter::repeat_with(fastrand::alphanumeric).take(32).collect()
    }
}

pub fn fetch_twitch_playlist(
    client_id: &Option<String>,
    auth_token: &Option<String>,
    args: &HlsArgs,
    agent: &Agent,
) -> Result<Url> {
    info!("Fetching playlist for channel {} (Twitch)", args.channel);
    let access_token = PlaybackAccessToken::new(client_id, auth_token, &args.channel, agent)?;
    let url = Url::parse_with_params(
        &format!("{}{}.m3u8", constants::TWITCH_HLS_BASE, args.channel),
        &[
            ("acmb", "e30="),
            ("allow_source", "true"),
            ("allow_audio_only", "true"),
            ("cdm", "wv"),
            ("fast_bread", "true"),
            ("playlist_include_framerate", "true"),
            ("player_backend", "mediaplayer"),
            ("reassignments_supported", "true"),
            ("supported_codecs", &args.codecs),
            ("transcode_mode", "cbr_v1"),
            ("p", &fastrand::u32(0..=9_999_999).to_string()),
            ("play_session_id", &access_token.play_session_id),
            ("sig", &access_token.signature),
            ("token", &access_token.token),
            ("player_version", "1.23.0"),
            ("warp", "true"),
        ],
    )?;

    parse_variant_playlist(
        &agent.get(&url)?.text().map_err(map_if_offline)?,
        &args.quality,
    )
}

pub fn fetch_proxy_playlist(servers: &[String], args: &HlsArgs, agent: &Agent) -> Result<Url> {
    info!("Fetching playlist for channel {} (proxy)", args.channel);
    let servers = servers
        .iter()
        .map(|s| {
            Url::parse_with_params(
                &s.replace("[channel]", &args.channel),
                &[
                    ("allow_source", "true"),
                    ("allow_audio_only", "true"),
                    ("fast_bread", "true"),
                    ("warp", "true"),
                    ("supported_codecs", &args.codecs),
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

            let mut request = match agent.get(s) {
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

    parse_variant_playlist(&playlist, &args.quality)
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

fn map_if_offline(error: anyhow::Error) -> anyhow::Error {
    if matches!(
        error.downcast_ref::<http::Error>(),
        Some(http::Error::NotFound(_))
    ) {
        return Error::Offline.into();
    }

    error
}
