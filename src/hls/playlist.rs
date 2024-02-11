use std::{
    collections::{vec_deque::Iter, VecDeque},
    env, iter,
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
};

pub struct MasterPlaylist {
    pub url: Url,
    pub low_latency: bool,
}

impl MasterPlaylist {
    pub fn new(args: &Args, agent: &Agent) -> Result<Self> {
        let low_latency = !args.no_low_latency;
        let mut master_playlist = if let Some(ref servers) = args.servers {
            Self::fetch_proxy_playlist(
                low_latency,
                servers,
                &args.codecs,
                &args.channel,
                &args.quality,
                agent,
            )?
        } else {
            Self::fetch_twitch_playlist(
                low_latency,
                &args.client_id,
                &args.auth_token,
                &args.codecs,
                &args.channel,
                &args.quality,
                agent,
            )?
        };

        master_playlist.low_latency = master_playlist.low_latency && low_latency;
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
        let url = format!(
            "{}{channel}.m3u8{}",
            constants::TWITCH_HLS_BASE,
            [
                "?acmb=e30%3D",
                "&allow_source=true",
                "&allow_audio_only=true",
                "&cdm=wv",
                &format!("&fast_bread={low_latency}"),
                "&playlist_include_framerate=true",
                "&player_backend=mediaplayer",
                "&reassignments_supported=true",
                &format!("&supported_codecs={codecs}"),
                "&transcode_mode=cbr_v1",
                &format!("&p={}", &fastrand::u32(0..=9_999_999).to_string()),
                &format!("&play_session_id={}", &access_token.play_session_id),
                &format!("&sig={}", &access_token.signature),
                &format!("&token={}", &http::url_encode(&access_token.token)),
                "&player_version=1.24.0-rc.1.3",
                &format!("&warp={low_latency}"),
                "&browser_family=firefox",
                &format!(
                    "&browser_version={}",
                    &constants::USER_AGENT[(constants::USER_AGENT.len() - 5)..],
                ),
                "&os_name=Windows",
                "&os_version=NT+10.0",
                "&platform=web",
            ]
            .join(""),
        );

        Self::parse_variant_playlist(
            agent.get(&url.into())?.text().map_err(map_if_offline)?,
            quality,
        )
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
        let playlist = servers
            .iter()
            .find_map(|s| {
                info!(
                    "Using server {}://{}",
                    s.split(':').next().unwrap_or("<unknown>"),
                    s.split('/')
                        .nth(2)
                        .unwrap_or_else(|| s.split('?').next().unwrap_or("<unknown>")),
                );

                let url = format!(
                    "{}{}",
                    &s.replace("[channel]", channel),
                    [
                        "?allow_source=true",
                        "&allow_audio_only=true",
                        &format!("&fast_bread={low_latency}"),
                        &format!("&warp={low_latency}"),
                        &format!("&supported_codecs={codecs}"),
                        "&platform=web",
                    ]
                    .join(""),
                );

                let mut request = match agent.get(&url.into()) {
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
                .into(),
            low_latency: playlist.contains("FUTURE=\"true\""),
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
    pub fn new(master_playlist: &MasterPlaylist, agent: &Agent) -> Result<Self> {
        let mut playlist = Self {
            header: Option::default(),

            segments: VecDeque::default(),
            sequence: usize::default(),
            added: usize::default(),

            request: agent.get(&master_playlist.url)?,

            debug_log_playlist: env::var_os("DEBUG_NO_PLAYLIST").is_none(),
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

                            debug!("All segments removed");
                        }
                    }

                    self.sequence = sequence;
                }
                "#EXT-X-MAP" => {
                    if self.header.is_none() {
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

    pub fn segments(&self) -> SegmentRange<'_> {
        if self.added == 0 {
            SegmentRange::Empty
        } else if self.added >= self.segments.len() {
            SegmentRange::Back(self.segments.back())
        } else {
            SegmentRange::Partial(self.segments.range(self.segments.len() - self.added..))
        }
    }

    pub fn last_duration(&self) -> Option<&Duration> {
        self.segments.iter().rev().find_map(|s| match s {
            Segment::Normal(duration, _) => Some(duration),
            _ => None,
        })
    }
}

pub enum SegmentRange<'a> {
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

        let mut request = agent.post(&constants::TWITCH_GQL_ENDPOINT.into(), &gql)?;
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
            let mut request = agent.get(&constants::TWITCH_OAUTH_ENDPOINT.into())?;
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

#[cfg(test)]
mod tests {
    use super::*;

    const MASTER_PLAYLIST: &'static str = r#"#EXT3MU
#EXT-X-TWITCH-INFO:NODE="...FUTURE="true"..."
#EXT-X-MEDIA:TYPE=VIDEO,GROUP-ID="chunked",NAME="1080p60 (source)",AUTOSELECT=YES,DEFAULT=YES
#EXT-X-STREAM-INF:BANDWIDTH=0,RESOLUTION=1920x1080,CODECS="avc1.64002A,mp4a.40.2",VIDEO="chunked",FRAME-RATE=60.000
http://1080p.invalid
#EXT-X-MEDIA:TYPE=VIDEO,GROUP-ID="720p60",NAME="720p60",AUTOSELECT=YES,DEFAULT=YES
#EXT-X-STREAM-INF:BANDWIDTH=0,RESOLUTION=1280x720,CODECS="avc1.4D401F,mp4a.40.2",VIDEO="720p60",FRAME-RATE=60.000
http://720p60.invalid
#EXT-X-MEDIA:TYPE=VIDEO,GROUP-ID="720p30",NAME="720p",AUTOSELECT=YES,DEFAULT=YES
#EXT-X-STREAM-INF:BANDWIDTH=0,RESOLUTION=1280x720,CODECS="avc1.4D401F,mp4a.40.2",VIDEO="720p30",FRAME-RATE=30.000
http://720p30.invalid
#EXT-X-MEDIA:TYPE=VIDEO,GROUP-ID="480p30",NAME="480p",AUTOSELECT=YES,DEFAULT=YES
#EXT-X-STREAM-INF:BANDWIDTH=0,RESOLUTION=852x480,CODECS="avc1.4D401F,mp4a.40.2",VIDEO="480p30",FRAME-RATE=30.000
http://480p.invalid
#EXT-X-MEDIA:TYPE=VIDEO,GROUP-ID="360p30",NAME="360p",AUTOSELECT=YES,DEFAULT=YES
#EXT-X-STREAM-INF:BANDWIDTH=0,RESOLUTION=640x360,CODECS="avc1.4D401F,mp4a.40.2",VIDEO="360p30",FRAME-RATE=30.000
http://360p.invalid
#EXT-X-MEDIA:TYPE=VIDEO,GROUP-ID="160p30",NAME="160p",AUTOSELECT=YES,DEFAULT=YES
#EXT-X-STREAM-INF:BANDWIDTH=0,RESOLUTION=284x160,CODECS="avc1.4D401F,mp4a.40.2",VIDEO="160p30",FRAME-RATE=30.000
http://160p.invalid
#EXT-X-MEDIA:TYPE=VIDEO,GROUP-ID="audio_only",NAME="audio_only",AUTOSELECT=NO,DEFAULT=NO
#EXT-X-STREAM-INF:BANDWIDTH=0,CODECS="mp4a.40.2",VIDEO="audio_only"
http://audio-only.invalid"#;

    #[test]
    fn parse_variant_playlist() {
        let qualities = [
            ("best", Some("1080p")),
            ("1080p", None),
            ("720p60", None),
            ("720p30", None),
            ("720p", Some("720p60")),
            ("480p", None),
            ("360p", None),
            ("160p", None),
            ("audio_only", Some("audio-only")),
        ];

        for (quality, host) in qualities {
            assert_eq!(
                MasterPlaylist::parse_variant_playlist(MASTER_PLAYLIST, quality)
                    .unwrap()
                    .url,
                format!("http://{}.invalid", host.unwrap_or(quality)).into(),
            );
        }
    }
}
