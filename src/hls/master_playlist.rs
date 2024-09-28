use std::{
    borrow::Cow,
    fmt::{self, Display, Formatter},
    fs, iter, mem,
};

use anyhow::{ensure, Context, Result};
use log::{debug, error, info};

use super::{Args, OfflineError};

use crate::{
    constants,
    http::{Agent, StatusError, Url},
};

#[derive(Default)]
pub struct MasterPlaylist {
    cache: Option<Cache>,
    quality: Option<String>,
    forced: bool,

    variant_playlists: Vec<VariantPlaylist>,
}

impl Display for MasterPlaylist {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        let mut iter = self.variant_playlists.iter();
        if let Some(playlist) = iter.next() {
            write!(f, "{} (best)", playlist.name)?;
        }

        for playlist in iter {
            write!(f, ", {}", playlist.name)?;
        }

        Ok(())
    }
}

impl MasterPlaylist {
    pub fn new(mut args: Args, agent: &Agent) -> Result<Self> {
        if let Some(url) = args.force_playlist_url.take() {
            info!("Using forced playlist URL");
            return Ok(Self::force_playlist_url(None, url));
        }

        let mut master_playlist = Self {
            cache: Cache::new(args.playlist_cache_dir.take(), &args.channel, &args.quality),
            quality: args.quality.take(),
            ..Default::default()
        };

        if let Some(url) = master_playlist.cache.as_ref().and_then(|c| c.get(agent)) {
            info!("Using cached playlist URL");
            return Ok(Self::force_playlist_url(Some(master_playlist), url));
        }

        info!("Fetching playlist for channel {}", &args.channel);
        let low_latency = !args.no_low_latency;
        master_playlist.variant_playlists = {
            if let Some(servers) = &args.servers {
                Self::fetch_proxy_playlist(
                    low_latency,
                    servers,
                    &args.codecs,
                    &args.channel,
                    agent,
                )?
            } else {
                Self::fetch_twitch_playlist(
                    low_latency,
                    args.client_id.take(),
                    args.auth_token.take(),
                    &args.codecs,
                    &args.channel,
                    agent,
                )?
            }
        };

        ensure!(
            !master_playlist.variant_playlists.is_empty(),
            "No variant playlist(s) found"
        );
        master_playlist.variant_playlists.dedup();

        Ok(master_playlist)
    }

    pub fn get_stream(&mut self) -> Option<Url> {
        let url = {
            let quality = self.quality.take()?;
            if self.forced || quality == "best" {
                mem::take(self.variant_playlists.first_mut()?).url
            } else {
                mem::take(
                    self.variant_playlists
                        .iter_mut()
                        .find(|v| v.name == quality)?,
                )
                .url
            }
        };

        if let Some(cache) = &mut self.cache {
            cache.create(&url);
        }

        Some(url)
    }

    fn fetch_twitch_playlist(
        low_latency: bool,
        client_id: Option<String>,
        auth_token: Option<String>,
        codecs: &str,
        channel: &str,
        agent: &Agent,
    ) -> Result<Vec<VariantPlaylist>> {
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
            &player_version={player_version}\
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
            player_version = constants::PLAYER_VERSION,
            browser_version = &constants::USER_AGENT[(constants::USER_AGENT.len() - 5)..],
        );

        Ok(Self::parse_variant_playlists(
            agent
                .get(url.into())?
                .text()
                .map_err(super::map_if_offline)?,
        ))
    }

    fn fetch_proxy_playlist(
        low_latency: bool,
        servers: &[Url],
        codecs: &str,
        channel: &str,
        agent: &Agent,
    ) -> Result<Vec<VariantPlaylist>> {
        let playlist = servers
            .iter()
            .find_map(|s| {
                info!(
                    "Using playlist proxy: {}://{}",
                    s.scheme.as_str(),
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
                    Err(e) if StatusError::is_not_found(&e) => {
                        error!("Playlist not found. Stream offline?");
                        None
                    }
                    Err(e) => {
                        error!("{e}");
                        None
                    }
                }
            })
            .ok_or(OfflineError)?;

        Ok(Self::parse_variant_playlists(&playlist))
    }

    fn parse_variant_playlists(playlist: &str) -> Vec<VariantPlaylist> {
        debug!("Master playlist:\n{playlist}");
        if playlist.contains("FUTURE=\"true\"") {
            info!("Low latency streaming");
        }

        playlist
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
            .collect()
    }

    fn force_playlist_url(master_playlist: Option<Self>, url: Url) -> Self {
        let mut master_playlist = master_playlist.unwrap_or_default();
        master_playlist.forced = true;
        master_playlist.variant_playlists = vec![VariantPlaylist {
            url,
            ..Default::default()
        }];

        master_playlist
    }
}

#[derive(Default)]
struct VariantPlaylist {
    url: Url,
    name: String,
}

impl PartialEq for VariantPlaylist {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

struct PlaybackAccessToken {
    token: String,
    signature: String,
    play_session_id: String,
}

impl PlaybackAccessToken {
    fn new(
        client_id: Option<String>,
        auth_token: Option<String>,
        channel: &str,
        agent: &Agent,
    ) -> Result<Self> {
        #[rustfmt::skip]
        let gql = format!(
        "{{\
            \"extensions\":{{\
                \"persistedQuery\":{{\
                    \"sha256Hash\":\"0828119ded1c13477966434e15800ff57ddacf13ba1911c129dc2200705b0712\",\
                    \"version\":1\
                }}\
            }},\
            \"operationName\":\"PlaybackAccessToken\",\
            \"variables\":{{\
                \"isLive\":true,\
                \"isVod\":false,\
                \"login\":\"{channel}\",\
                \"playerType\":\"site\",\
                \"vodID\":\"\"\
            }}\
        }}");

        let mut request = agent.post(constants::TWITCH_GQL_ENDPOINT.into(), gql)?;
        request.header("Content-Type: text/plain;charset=UTF-8")?;
        request.header(&format!("X-Device-ID: {}", &Self::gen_id()))?;

        if let Some(auth_token) = &auth_token {
            request.header(&format!("Authorization: OAuth {auth_token}"))?;
        }

        request.header(&format!(
            "Client-Id: {}",
            Self::choose_client_id(client_id, auth_token, agent)?
        ))?;

        let response = request.text()?;
        debug!("GQL response: {response}");

        Ok(Self {
            token: {
                let start = response.find(r#"{\"adblock\""#).ok_or(OfflineError)?;
                let end = response.find(r#"","signature""#).ok_or(OfflineError)?;

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
        client_id: Option<String>,
        auth_token: Option<String>,
        agent: &Agent,
    ) -> Result<Cow<'static, str>> {
        let client_id = {
            if let Some(client_id) = client_id {
                Cow::Owned(client_id)
            } else if let Some(auth_token) = auth_token {
                let mut request = agent.get(constants::TWITCH_OAUTH_ENDPOINT.into())?;
                request.header(&format!("Authorization: OAuth {auth_token}"))?;

                Cow::Owned(
                    request
                        .text()?
                        .split_once(r#""client_id":""#)
                        .context("Failed to parse client id in GQL response")?
                        .1
                        .chars()
                        .take(30)
                        .collect(),
                )
            } else {
                Cow::Borrowed(constants::DEFAULT_CLIENT_ID)
            }
        };

        Ok(client_id)
    }

    fn gen_id() -> String {
        iter::repeat_with(fastrand::alphanumeric).take(32).collect()
    }
}

struct Cache {
    path: String,
}

impl Cache {
    fn new(dir: Option<String>, channel: &str, quality: &Option<String>) -> Option<Self> {
        if let Some(dir) = dir {
            if let Some(quality) = quality {
                match fs::metadata(&dir) {
                    Ok(metadata) if metadata.is_dir() && !metadata.permissions().readonly() => {
                        return Some(Self {
                            path: format!("{dir}/{channel}-{quality}"),
                        });
                    }
                    Err(e) => error!("Failed to open playlist cache directory: {e}"),
                    _ => error!("Playlist cache path is not writable or is not a directory"),
                }
            }
        }

        None
    }

    fn get(&self, agent: &Agent) -> Option<Url> {
        debug!("Reading playlist cache: {}", self.path);

        let url: Url = fs::read_to_string(&self.path).ok()?.trim_end().into();
        if !agent.exists(url.clone()) {
            debug!("Removing playlist cache: {}", self.path);
            if let Err(e) = fs::remove_file(&self.path) {
                error!("Failed to remove playlist cache: {e}");
            }

            return None;
        }

        Some(url)
    }

    fn create(&self, url: &Url) {
        debug!("Creating playlist cache: {}", self.path);
        if let Err(e) = fs::write(&self.path, url.as_str()) {
            error!("Failed to create playlist cache: {e}");
        }
    }
}
