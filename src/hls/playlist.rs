use std::{env, iter};

use anyhow::{Context, Result};
use log::{debug, error, info};

use super::{
    segment::{Header, Segment},
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
                &format!("&fast_bread={}", &low_latency.to_string()),
                "&playlist_include_framerate=true",
                "&player_backend=mediaplayer",
                "&reassignments_supported=true",
                &format!("&supported_codecs={codecs}"),
                "&transcode_mode=cbr_v1",
                &format!("&p={}", &fastrand::u32(0..=9_999_999).to_string()),
                &format!("&play_session_id={}", &access_token.play_session_id),
                &format!("&sig={}", &access_token.signature),
                &format!("&token={}", &access_token.token),
                "&player_version=1.24.0-rc.1.3",
                &format!("&warp={}", &low_latency.to_string()),
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
            &agent.get(&url.into())?.text().map_err(map_if_offline)?,
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
                        &format!("&fast_bread={}", &low_latency.to_string()),
                        &format!("&warp={}", &low_latency.to_string()),
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
                .into(),
            low_latency: playlist.contains("FUTURE=\"true\""),
        })
    }
}

pub struct MediaPlaylist {
    playlist: String,
    request: TextRequest,

    playlist_debug: bool,
}

impl MediaPlaylist {
    pub fn new(master_playlist: &MasterPlaylist, agent: &Agent) -> Result<Self> {
        let mut playlist = Self {
            playlist: String::default(),
            request: agent.get(&master_playlist.url)?,
            playlist_debug: env::var_os("DEBUG_NO_PLAYLIST").is_none(),
        };

        playlist.reload()?;
        Ok(playlist)
    }

    pub fn reload(&mut self) -> Result<()> {
        debug!("----------RELOADING----------");

        self.playlist = self.request.text().map_err(map_if_offline)?;
        if self.playlist_debug {
            debug!("Playlist:\n{}", self.playlist);
        }

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
        Ok(self.playlist.parse::<Header>()?.0)
    }

    pub fn segments(&self) -> Result<Vec<Segment>> {
        let mut lines = self.playlist.lines();

        let mut segments = Vec::new();
        while let Some(line) = lines.next() {
            if line.starts_with("#EXTINF") {
                if let Some(url) = lines.next() {
                    segments.push(Segment::Normal(line.parse()?, url.into()));
                }
            } else if Self::is_prefetch_segment(line) {
                segments.push(Segment::NextPrefetch(
                    self.playlist
                        .lines()
                        .rev()
                        .find(|l| l.starts_with("#EXTINF"))
                        .context("Failed to find prefetch segment duration")?
                        .parse()?,
                    Self::split_prefetch_url(line)?,
                ));

                if let Some(line) = lines.next() {
                    if Self::is_prefetch_segment(line) {
                        segments.push(Segment::NewestPrefetch(Self::split_prefetch_url(line)?));
                    }
                }
            }
        }

        Ok(segments)
    }

    fn is_prefetch_segment(line: &str) -> bool {
        line.starts_with("#EXT-X-TWITCH-PREFETCH")
    }

    fn split_prefetch_url(line: &str) -> Result<Url> {
        Ok(line
            .split_once(':')
            .context("Failed to parse prefetch URL")?
            .1
            .into())
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

                request.encode(&response[start..end].replace('\\', ""))
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
pub mod tests {
    use super::super::tests::PLAYLIST;
    use super::*;

    pub const MASTER_PLAYLIST: &'static str = r#"#EXT3MU
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

    pub fn create_playlist() -> MediaPlaylist {
        MediaPlaylist {
            playlist: PLAYLIST.to_owned(),
            request: Agent::new(&http::Args::default())
                .unwrap()
                .get(&"http://playlist.invalid".into())
                .unwrap(),
            playlist_debug: true,
        }
    }

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
