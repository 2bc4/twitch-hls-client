use std::{
    collections::{vec_deque::Iter, VecDeque},
    env,
    fmt::{self, Display, Formatter},
    iter, mem,
};

use anyhow::{ensure, Context, Result};
use log::{debug, error, info};

use super::{
    segment::{Duration, Segment},
    Args, Error,
};

use crate::{
    constants,
    http::{self, Agent, TextRequest, Url},
    logger,
};

#[derive(Default)]
pub struct VariantPlaylist {
    pub url: Url,
    name: String,
}

pub struct MasterPlaylist {
    variant_playlists: Vec<VariantPlaylist>,
}

impl Display for MasterPlaylist {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        let last_idx = self.variant_playlists.len() - 1;
        for (idx, playlist) in self.variant_playlists.iter().enumerate() {
            if idx == 0 {
                write!(f, "{} (best),", playlist.name)?;
                continue;
            }

            write!(f, " {}", playlist.name)?;
            if idx != last_idx {
                write!(f, ",")?;
            }
        }

        Ok(())
    }
}

impl MasterPlaylist {
    pub fn new(args: &Args, agent: &Agent) -> Result<Self> {
        let low_latency = !args.no_low_latency;
        let master_playlist = if let Some(ref servers) = args.servers {
            Self::fetch_proxy_playlist(low_latency, servers, &args.codecs, &args.channel, agent)?
        } else {
            Self::fetch_twitch_playlist(
                low_latency,
                &args.client_id,
                &args.auth_token,
                &args.codecs,
                &args.channel,
                agent,
            )?
        };

        Ok(master_playlist)
    }

    pub fn find(&mut self, name: &str) -> Option<VariantPlaylist> {
        if name == "best" {
            return Some(mem::take(self.variant_playlists.first_mut()?));
        }

        Some(mem::take(
            self.variant_playlists.iter_mut().find(|v| v.name == name)?,
        ))
    }

    fn fetch_twitch_playlist(
        low_latency: bool,
        client_id: &Option<String>,
        auth_token: &Option<String>,
        codecs: &str,
        channel: &str,
        agent: &Agent,
    ) -> Result<Self> {
        info!("Fetching playlist for channel {channel}");
        let access_token = PlaybackAccessToken::new(client_id, auth_token, channel, agent)?;
        let url = format!(
            "{base_url}{channel}.m3u8\
            ?acmb=e30%3D\
            &allow_source=true\
            &allow_audio_only=true\
            &cdm=wv\
            &fast_bread={low_latency}\
            &playlist_include_framerate=true\
            &player_backend=mediaplayer\
            &reassignments_supported=true\
            &supported_codecs={codecs}\
            &transcode_mode=cbr_v1\
            &p={p}\
            &play_session_id={play_session_id}\
            &sig={sig}\
            &token={token}\
            &player_version=1.24.0-rc.1.3\
            &warp={low_latency}\
            &browser_family=firefox\
            &browser_version={browser_version}\
            &os_name=Windows\
            &os_version=NT+10.0\
            &platform=web",
            base_url = constants::TWITCH_HLS_BASE,
            p = fastrand::u32(0..=9_999_999),
            play_session_id = &access_token.play_session_id,
            sig = access_token.signature,
            token = access_token.token,
            browser_version = &constants::USER_AGENT[(constants::USER_AGENT.len() - 5)..],
        );

        Self::parse_variant_playlists(agent.get(url.into())?.text().map_err(map_if_offline)?)
    }

    fn fetch_proxy_playlist(
        low_latency: bool,
        servers: &[Url],
        codecs: &str,
        channel: &str,
        agent: &Agent,
    ) -> Result<Self> {
        info!("Fetching playlist for channel {channel} (proxy)");
        let playlist = servers
            .iter()
            .find_map(|s| {
                info!(
                    "Using server {}://{}",
                    s.scheme().unwrap_or("<unknown>"),
                    s.host().unwrap_or("<unknown>"),
                );

                let url = format!(
                    "{}?allow_source=true\
                    &allow_audio_only=true\
                    &fast_bread={low_latency}\
                    &warp={low_latency}\
                    &supported_codecs={codecs}\
                    &platform=web",
                    &s.replace("[channel]", channel),
                );

                let mut request = match agent.get(url.into()) {
                    Ok(request) => request,
                    Err(e) => {
                        error!("{e}");
                        return None;
                    }
                };

                match request.text() {
                    Ok(playlist_url) => Some(playlist_url.to_owned()),
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

        Self::parse_variant_playlists(&playlist)
    }

    fn parse_variant_playlists(playlist: &str) -> Result<Self> {
        debug!("Master playlist:\n{playlist}");
        if playlist.contains("FUTURE=\"true\"") {
            info!("Low latency streaming");
        }

        Ok(Self {
            variant_playlists: playlist
                .lines()
                .filter(|l| l.starts_with("#EXT-X-MEDIA"))
                .zip(playlist.lines().filter(|l| l.starts_with("http")))
                .filter_map(|(line, url)| {
                    Some(VariantPlaylist {
                        name: line
                            .split_once("NAME=\"")
                            .map(|s| s.1.split('"'))
                            .and_then(|mut s| s.next())
                            .map(|s| s.replace(" (source)", ""))?,
                        url: url.into(),
                    })
                })
                .collect(),
        })
    }
}

pub struct MediaPlaylist {
    pub header: Option<Url>, //used for av1/hevc streams

    segments: VecDeque<Segment>,
    sequence: usize,
    added: usize,

    request: TextRequest,

    debug_log_playlist: bool,
}

impl MediaPlaylist {
    pub fn new(url: Url, agent: &Agent) -> Result<Self> {
        let mut playlist = Self {
            header: Option::default(),

            segments: VecDeque::with_capacity(16),
            sequence: usize::default(),
            added: usize::default(),

            request: agent.get(url)?,

            debug_log_playlist: logger::is_debug() && env::var_os("DEBUG_NO_PLAYLIST").is_none(),
        };

        playlist.reload()?;
        Ok(playlist)
    }

    pub fn reload(&mut self) -> Result<()> {
        debug!("----------RELOADING----------");
        let playlist = self.request.text().map_err(map_if_offline)?;
        if self.debug_log_playlist {
            debug!("Playlist:\n{playlist}");
        }

        if playlist
            .lines()
            .next_back()
            .is_some_and(|l| l.starts_with("#EXT-X-ENDLIST"))
        {
            return Err(Error::Offline.into());
        }

        let mut prefetch_removed = 0;
        for _ in 0..2 {
            if let Some(segment) = self.segments.back() {
                match segment {
                    Segment::NextPrefetch(_) | Segment::NewestPrefetch(_) => {
                        self.segments.pop_back();
                        prefetch_removed += 1;
                    }
                    Segment::Normal(_, _) => (),
                }
            }
        }

        let mut prev_segment_count = self.segments.len();
        let mut total_segments = 0;
        let mut lines = playlist.lines().peekable();
        while let Some(line) = lines.next() {
            let Some(split) = line.split_once(':') else {
                continue;
            };

            match split.0 {
                "#EXT-X-MEDIA-SEQUENCE" => {
                    let sequence = split.1.parse()?;
                    ensure!(sequence >= self.sequence, "Sequence went backwards");

                    if sequence > 0 {
                        let removed = sequence - self.sequence;
                        if removed < self.segments.len() {
                            self.segments.drain(..removed);
                            prev_segment_count = self.segments.len();

                            debug!("Segments removed: {removed}");
                        } else {
                            self.segments.clear();
                            prev_segment_count = 0;
                            prefetch_removed = 0;

                            debug!("All segments removed");
                        }
                    }

                    self.sequence = sequence;
                }
                "#EXT-X-MAP" if self.header.is_none() => {
                    self.header = Some(
                        split
                            .1
                            .split_once('=')
                            .context("Failed to parse segment header")?
                            .1
                            .replace('"', "")
                            .into(),
                    );
                }
                "#EXTINF" => {
                    total_segments += 1;
                    if total_segments > prev_segment_count {
                        if let Some(url) = lines.next() {
                            self.segments
                                .push_back(Segment::Normal(split.1.parse()?, url.into()));
                        }
                    }
                }
                "#EXT-X-TWITCH-PREFETCH" => {
                    total_segments += 1;
                    if total_segments > prev_segment_count {
                        if lines.peek().is_some() {
                            self.segments
                                .push_back(Segment::NextPrefetch(split.1.into()));
                        } else {
                            self.segments
                                .push_back(Segment::NewestPrefetch(split.1.into()));
                        }
                    }
                }
                _ => continue,
            }
        }

        self.added = total_segments - (prev_segment_count + prefetch_removed);
        debug!("Segments added: {}", self.added);

        Ok(())
    }

    pub fn segments(&self) -> QueueRange<'_> {
        if self.added == 0 {
            QueueRange::Empty
        } else if self.added == self.segments.len() {
            QueueRange::Back(self.segments.back())
        } else {
            QueueRange::Partial(self.segments.range(self.segments.len() - self.added..))
        }
    }

    pub fn last_duration(&self) -> Option<&Duration> {
        self.segments.iter().rev().find_map(|s| match s {
            Segment::Normal(duration, _) => Some(duration),
            _ => None,
        })
    }
}

pub enum QueueRange<'a> {
    Partial(Iter<'a, Segment>),
    Back(Option<&'a Segment>),
    Empty,
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

        let mut request = agent.post(constants::TWITCH_GQL_ENDPOINT.into(), gql)?;
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
            let mut request = agent.get(constants::TWITCH_OAUTH_ENDPOINT.into())?;
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
