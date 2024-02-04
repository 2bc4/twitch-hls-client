use std::iter;

use anyhow::{Context, Result};
use log::{debug, error, info};
use url::Url;

use super::{
    segment::{PrefetchSegment, Segment, SegmentDuration, SegmentHeader},
    Args, Error,
};
use crate::{
    constants,
    http::{self, Agent, TextRequest},
};

pub struct MasterPlaylist {
    pub url: Url,
    pub low_latency: bool,
}

impl MasterPlaylist {
    pub fn new(args: &Args, agent: &Agent) -> Result<Self> {
        let mut master_playlist = if let Some(ref servers) = args.servers {
            Self::fetch_proxy_playlist(
                args.low_latency,
                servers,
                &args.codecs,
                &args.channel,
                &args.quality,
                agent,
            )?
        } else {
            Self::fetch_twitch_playlist(
                args.low_latency,
                &args.client_id,
                &args.auth_token,
                &args.codecs,
                &args.channel,
                &args.quality,
                agent,
            )?
        };

        master_playlist.low_latency = master_playlist.low_latency && args.low_latency;
        if master_playlist.low_latency {
            info!("Low latency streaming");
        }

        Ok(master_playlist)
    }

    fn fetch_twitch_playlist(
        low_latency: bool,
        client_id: &Option<String>,
        auth_token: &Option<String>,
        codecs: &str,
        channel: &str,
        quality: &str,
        agent: &Agent,
    ) -> Result<Self> {
        info!("Fetching playlist for channel {channel}");
        let access_token = PlaybackAccessToken::new(client_id, auth_token, channel, agent)?;
        let url = Url::parse_with_params(
            &format!("{}{}.m3u8", constants::TWITCH_HLS_BASE, channel),
            &[
                ("acmb", "e30="),
                ("allow_source", "true"),
                ("allow_audio_only", "true"),
                ("cdm", "wv"),
                ("fast_bread", &low_latency.to_string()),
                ("playlist_include_framerate", "true"),
                ("player_backend", "mediaplayer"),
                ("reassignments_supported", "true"),
                ("supported_codecs", &codecs),
                ("transcode_mode", "cbr_v1"),
                ("p", &fastrand::u32(0..=9_999_999).to_string()),
                ("play_session_id", &access_token.play_session_id),
                ("sig", &access_token.signature),
                ("token", &access_token.token),
                ("player_version", "1.24.0-rc.1.3"),
                ("warp", &low_latency.to_string()),
                ("browser_family", "firefox"),
                (
                    "browser_version",
                    &constants::USER_AGENT[(constants::USER_AGENT.len() - 5)..],
                ),
                ("os_name", "Windows"),
                ("os_version", "NT 10.0"),
                ("platform", "web"),
            ],
        )?;

        Self::parse_variant_playlist(&agent.get(&url)?.text().map_err(map_if_offline)?, quality)
    }

    fn fetch_proxy_playlist(
        low_latency: bool,
        servers: &[String],
        codecs: &str,
        channel: &str,
        quality: &str,
        agent: &Agent,
    ) -> Result<Self> {
        info!("Fetching playlist for channel {channel} (proxy)");
        let servers = servers
            .iter()
            .map(|s| {
                Url::parse_with_params(
                    &s.replace("[channel]", channel),
                    &[
                        ("allow_source", "true"),
                        ("allow_audio_only", "true"),
                        ("fast_bread", &low_latency.to_string()),
                        ("warp", &low_latency.to_string()),
                        ("supported_codecs", &codecs),
                        ("platform", "web"),
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

    fn parse_variant_playlist(playlist: &str, quality: &str) -> Result<Self> {
        debug!("Master playlist:\n{playlist}");
        Ok(Self {
            url: playlist
                .lines()
                .skip_while(|s| {
                    !(s.contains("#EXT-X-MEDIA") && (s.contains(quality) || quality == "best"))
                })
                .nth(2)
                .context("Invalid quality or malformed master playlist")?
                .parse()?,
            low_latency: playlist.contains("FUTURE=\"true\""),
        })
    }
}

pub struct MediaPlaylist {
    playlist: String,
    request: TextRequest,
}

impl MediaPlaylist {
    pub fn new(master_playlist: &MasterPlaylist, agent: &Agent) -> Result<Self> {
        let mut playlist = Self {
            playlist: String::default(),
            request: agent.get(&master_playlist.url)?,
        };

        playlist.reload()?;
        Ok(playlist)
    }

    pub fn reload(&mut self) -> Result<()> {
        debug!("----------RELOADING----------");

        self.playlist = self.request.text().map_err(map_if_offline)?;
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

    pub fn header(&self) -> Result<Option<Url>> {
        Ok(self.playlist.parse::<SegmentHeader>()?.0)
    }

    pub fn prefetch_url(&self, prefetch_segment: PrefetchSegment) -> Result<Url> {
        Ok(prefetch_segment.parse(&self.playlist)?)
    }

    pub fn last_duration(&self) -> Result<SegmentDuration> {
        self.playlist
            .lines()
            .rev()
            .find(|l| l.starts_with("#EXTINF"))
            .context("Failed to get prefetch segment duration")?
            .parse()
    }

    pub fn segments(&self) -> Result<Vec<Segment>> {
        let mut lines = self.playlist.lines();

        let mut segments = Vec::new();
        while let Some(extinf) = lines.next() {
            if extinf.starts_with("#EXTINF") && !extinf.contains("Amazon") {
                if let Some(url) = lines.next() {
                    segments.push(Segment::new(extinf, url)?);
                }
            }
        }

        Ok(segments)
    }

    pub fn has_ad(&self) -> bool {
        self.playlist
            .lines()
            .rev()
            .find(|l| l.starts_with("#EXTINF"))
            .is_some_and(|l| l.contains("Amazon"))
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

fn map_if_offline(error: anyhow::Error) -> anyhow::Error {
    if matches!(
        error.downcast_ref::<http::Error>(),
        Some(http::Error::NotFound(_))
    ) {
        return Error::Offline.into();
    }

    error
}
