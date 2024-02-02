#![allow(clippy::unnecessary_wraps)] //function pointers

use std::{fmt, iter, str::FromStr, thread, time::Duration};

use anyhow::{ensure, Context, Result};
use log::{debug, error, info};
use url::Url;

use crate::{
    args::{ArgParse, Parser},
    constants,
    http::{self, Agent, TextRequest},
};

#[derive(Debug)]
pub enum Error {
    Offline,
    Advertisement,
    NotLowLatency(Url),
}

impl std::error::Error for Error {}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Offline => write!(f, "Stream is offline or unavailable"),
            Self::Advertisement => write!(f, "Encountered an embedded advertisement"),
            Self::NotLowLatency(_) => write!(f, "Stream is not low latency"),
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
    pub channel: String,
    pub quality: String,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            codecs: "av1,h265,h264".to_owned(),
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

pub struct MediaPlaylist {
    playlist: String,
    request: TextRequest,
}

impl MediaPlaylist {
    pub fn new(args: &Args, agent: &Agent) -> Result<Self> {
        let url = if let Some(ref servers) = args.servers {
            Self::fetch_proxy_playlist(servers, &args.codecs, &args.channel, &args.quality, agent)
        } else {
            Self::fetch_twitch_playlist(
                &args.client_id,
                &args.auth_token,
                &args.codecs,
                &args.channel,
                &args.quality,
                agent,
            )
        }?;

        let mut playlist = Self {
            playlist: String::default(),
            request: agent.get(&url)?,
        };

        playlist.reload()?;
        Ok(playlist)
    }

    pub fn reload(&mut self) -> Result<()> {
        self.playlist = self.request.text().map_err(Self::map_if_offline)?;
        debug!("Playlist:\n{}", self.playlist);

        if self
            .playlist
            .lines()
            .next_back()
            .unwrap_or_default()
            .starts_with("#EXT-X-ENDLIST")
        {
            return Err(Error::Offline.into());
        }

        Ok(())
    }

    //Used for av1/hevc streams
    pub fn header(&mut self) -> Result<Option<Url>> {
        let header_url = self
            .playlist
            .lines()
            .find(|s| s.starts_with("#EXT-X-MAP"))
            .and_then(|s| s.split_once('='))
            .map(|s| s.1.replace('"', ""));

        if let Some(header_url) = header_url {
            return Ok(Some(
                header_url
                    .parse()
                    .context("Failed to parse segment header URL")?,
            ));
        }

        Ok(None)
    }

    pub fn prefetch_url(&mut self, prefetch_segment: PrefetchSegment) -> Result<Url> {
        Ok(prefetch_segment.parse(&self.playlist)?)
    }

    pub fn duration(&self) -> Result<SegmentDuration> {
        self.playlist.parse()
    }

    pub fn url(&mut self) -> Result<Url> {
        self.request.url()
    }

    fn fetch_twitch_playlist(
        client_id: &Option<String>,
        auth_token: &Option<String>,
        codecs: &str,
        channel: &str,
        quality: &str,
        agent: &Agent,
    ) -> Result<Url> {
        info!("Fetching playlist for channel {channel}");
        let access_token = PlaybackAccessToken::new(client_id, auth_token, channel, agent)?;
        let url = Url::parse_with_params(
            &format!("{}{}.m3u8", constants::TWITCH_HLS_BASE, channel),
            &[
                ("acmb", "e30="),
                ("allow_source", "true"),
                ("allow_audio_only", "true"),
                ("cdm", "wv"),
                ("fast_bread", "true"),
                ("playlist_include_framerate", "true"),
                ("player_backend", "mediaplayer"),
                ("reassignments_supported", "true"),
                ("supported_codecs", &codecs),
                ("transcode_mode", "cbr_v1"),
                ("p", &fastrand::u32(0..=9_999_999).to_string()),
                ("play_session_id", &access_token.play_session_id),
                ("sig", &access_token.signature),
                ("token", &access_token.token),
                ("player_version", "1.23.0"),
                ("warp", "true"),
            ],
        )?;

        Self::parse_variant_playlist(
            &agent.get(&url)?.text().map_err(Self::map_if_offline)?,
            quality,
        )
    }

    fn fetch_proxy_playlist(
        servers: &[String],
        codecs: &str,
        channel: &str,
        quality: &str,
        agent: &Agent,
    ) -> Result<Url> {
        info!("Fetching playlist for channel {channel} (proxy)");
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
                        ("supported_codecs", &codecs),
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

        Self::parse_variant_playlist(&playlist, quality)
    }

    fn parse_variant_playlist(master_playlist: &str, quality: &str) -> Result<Url> {
        debug!("Master playlist:\n{master_playlist}");
        let playlist_url = master_playlist
            .lines()
            .skip_while(|s| {
                !(s.contains("#EXT-X-MEDIA") && (s.contains(quality) || quality == "best"))
            })
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
}

#[derive(Copy, Clone, PartialEq)]
pub enum PrefetchSegment {
    Newest,
    Next,
}

impl PrefetchSegment {
    fn parse(self, playlist: &str) -> Result<Url, Error> {
        playlist
            .lines()
            .rev()
            .filter(|s| s.starts_with("#EXT-X-TWITCH-PREFETCH"))
            .nth(self as usize)
            .and_then(|s| s.split_once(':'))
            .map(|s| s.1)
            .ok_or(Error::Advertisement)?
            .parse()
            .or(Err(Error::Advertisement))
    }
}

pub struct SegmentDuration(Duration);

impl FromStr for SegmentDuration {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(
            Duration::try_from_secs_f32(
                s.lines()
                    .rev()
                    .find(|s| s.starts_with("#EXTINF"))
                    .and_then(|s| s.split_once(':'))
                    .and_then(|s| s.1.split_once(','))
                    .map(|s| s.0.parse())
                    .context("Invalid segment duration")??,
            )
            .context("Failed to parse segment duration")?,
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
                let start = response.find(r#"{\"adblock\""#).ok_or(Error::Offline)?;
                let end = response.find(r#"","signature""#).ok_or(Error::Offline)?;

                response[start..end].replace('\\', "")
            },
            signature: response
                .split_once(r#""signature":""#)
                .context("Failed to parse signature in GQL response")?
                .1
                .chars()
                .take(40)
                .collect(),
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
                .collect()
        } else {
            constants::DEFAULT_CLIENT_ID.to_owned()
        };

        Ok(client_id)
    }

    fn gen_id() -> String {
        iter::repeat_with(fastrand::alphanumeric).take(32).collect()
    }
}
