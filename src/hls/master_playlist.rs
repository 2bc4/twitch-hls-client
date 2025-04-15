use std::{
    borrow::Cow,
    fmt::{self, Display, Formatter},
    ops::{Deref, DerefMut},
    str::{self, Utf8Error},
};

use anyhow::{Context, Result};
use getrandom::getrandom;
use log::{debug, error, info};

use super::{Args, OfflineError, cache::Cache, map_if_offline};

use crate::{
    constants,
    http::{Agent, Connection, Method, StatusError, Url},
};

pub fn fetch_playlist(mut args: Args, agent: &Agent) -> Result<Option<Connection>> {
    if let Some(url) = args.force_playlist_url.take() {
        info!("Using forced playlist URL");
        return Ok(Some(Connection::new(url, agent.text())));
    }

    let cache = Cache::new(&args.playlist_cache_dir, &args.channel, &args.quality);
    if let Some(conn) = cache.as_ref().and_then(|c| c.get(agent)) {
        info!("Using cached playlist URL");
        return Ok(Some(conn));
    }

    info!("Fetching playlist for channel {}", &args.channel);
    let playlist = if let Some(servers) = &args.servers {
        fetch_proxy_playlist(
            !args.no_low_latency,
            servers,
            &args.codecs,
            &args.channel,
            agent,
        )?
    } else {
        let response = fetch_twitch_gql(
            args.client_id.take(),
            args.auth_token.take(),
            &args.channel,
            agent,
        )?;

        fetch_twitch_playlist(
            &response,
            !args.no_low_latency,
            &args.codecs,
            &args.channel,
            agent,
        )?
    };

    let Some(url) = choose_stream(&playlist, &args.quality, args.print_streams) else {
        print_streams(&playlist);
        return Ok(None);
    };

    if let Some(cache) = &cache {
        cache.create(&url);
    }

    Ok(Some(Connection::new(url, agent.text())))
}

fn fetch_twitch_gql(
    client_id: Option<String>,
    auth_token: Option<String>,
    channel: &str,
    agent: &Agent,
) -> Result<String> {
    const GQL_LEN_WITHOUT_CHANNEL: usize = 249;

    let mut client_id_buf = ArrayString::<30>::new();
    let client_id = choose_client_id(&mut client_id_buf, client_id, &auth_token, agent)?;

    let mut request = agent.text();
    request.text_fmt(
        Method::Post,
        &constants::TWITCH_GQL_ENDPOINT.into(),
        format_args!(
            "Content-Type: text/plain;charset=UTF-8\r\n\
             X-Device-ID: {device_id}\r\n\
             Client-ID: {client_id}\r\n\
             {auth_token_head}{auth_token}{auth_token_tail}\
             Content-Length: {content_length}\r\n\
             \r\n\
             {{\
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
             }}",
             device_id = ArrayString::<32>::random()?,
             content_length = GQL_LEN_WITHOUT_CHANNEL + channel.len(),
             auth_token_head = if auth_token.is_some() { "Authorization: OAuth " } else { "" },
             auth_token_tail = if auth_token.is_some() { "\r\n" } else { "" },
             auth_token = auth_token.unwrap_or_default(),
        )
    )?;

    let mut response = request.take();
    response.retain(|c| c != '\\');

    debug!("GQL response: {response}");
    if response.contains(r#"streamPlaybackAccessToken":null"#) {
        return Err(OfflineError.into());
    }

    Ok(response)
}

fn fetch_twitch_playlist(
    gql_response: &str,
    low_latency: bool,
    codecs: &str,
    channel: &str,
    agent: &Agent,
) -> Result<String> {
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
        p = {
            let mut buf = [0u8; 4];
            getrandom(&mut buf)?;

            u32::from_be_bytes(buf) % 9_999_999
        },
        play_session_id = ArrayString::<32>::random()?,
        sig = {
            const SIGNATURE_LEN: usize = 40;
            const TOKEN: &str = r#""signature":""#;

            let start = gql_response
                .find(TOKEN)
                .context("Failed to find signature in GQL response")?
                + TOKEN.len();

            gql_response
                .get(start..start + SIGNATURE_LEN)
                .context("Invalid signature in GQL response")?
        },
        token = {
            let start = gql_response.find(r#"{"adblock""#).ok_or(OfflineError)?;
            let end = gql_response.find(r#"","signature""#).ok_or(OfflineError)?;

            &gql_response[start..end]
        },
        player_version = constants::PLAYER_VERSION,
        browser_version = &constants::USER_AGENT[(constants::USER_AGENT.len() - 5)..],
    )
    .into();

    let mut request = agent.text();
    request.text(Method::Get, &url).map_err(map_if_offline)?;

    Ok(request.take())
}

fn fetch_proxy_playlist(
    low_latency: bool,
    servers: &[Url],
    codecs: &str,
    channel: &str,
    agent: &Agent,
) -> Result<String, OfflineError> {
    let mut request = agent.text();
    for server in servers {
        info!(
            "Using playlist proxy: {}://{}",
            server.scheme,
            server.host().unwrap_or("<unknown>"),
        );

        let url = format!(
            "{}?allow_source=true\
            &allow_audio_only=true\
            &fast_bread={low_latency}\
            &warp={low_latency}\
            &supported_codecs={codecs}\
            &platform=web",
            &server.replace("[channel]", channel),
        )
        .into();

        match request.text(Method::Get, &url) {
            Ok(_) => break,
            Err(e) if StatusError::is_not_found(&e) => error!("Server returned stream offline"),
            Err(e) => error!("{e}"),
        }
    }

    let playlist = request.take();
    if playlist.is_empty() {
        return Err(OfflineError);
    }

    Ok(playlist)
}

fn choose_stream(playlist: &str, quality: &Option<String>, should_print: bool) -> Option<Url> {
    debug!("Master playlist:\n{playlist}");
    let (Some(quality), false) = (quality, should_print) else {
        return None;
    };

    let mut iter = playlist_iter(playlist);
    if quality == "best" {
        return Some(iter.next()?.1.into());
    }

    iter.find(|(name, _)| name == quality)
        .map(|(_, url)| url.into())
}

fn playlist_iter(playlist: &str) -> impl Iterator<Item = (&str, &str)> {
    playlist
        .lines()
        .filter(|l| l.starts_with("#EXT-X-MEDIA"))
        .zip(playlist.lines().filter(|l| l.starts_with("http")))
        .filter_map(|(line, url)| {
            Some((
                line.split_once("NAME=\"")
                    .map(|s| s.1.split('"'))
                    .and_then(|mut s| s.next())
                    .map(|s| s.strip_suffix(" (source)").unwrap_or(s))?,
                url,
            ))
        })
}

fn print_streams(playlist: &str) {
    let mut iter = playlist_iter(playlist);
    if let Some((name, _)) = iter.next() {
        print!("Available streams: {name} (best)");
    }

    for (name, _) in iter {
        print!(", {name}");
    }
    println!();
}

fn choose_client_id<'a>(
    buf: &'a mut ArrayString<30>,
    client_id: Option<String>,
    auth_token: &Option<String>,
    agent: &Agent,
) -> Result<Cow<'a, str>> {
    if let Some(client_id) = client_id {
        Ok(Cow::Owned(client_id))
    } else if let Some(auth_token) = auth_token {
        let mut request = agent.text();
        let response = request.text_fmt(
            Method::Get,
            &constants::TWITCH_OAUTH_ENDPOINT.into(),
            format_args!("Authorization: OAuth {auth_token}\r\n\r\n"),
        )?;

        response
            .split_once(r#""client_id":""#)
            .context("Failed to parse client ID in GQL response")?
            .1
            .chars()
            .take(30)
            .zip(buf.iter_mut())
            .for_each(|(src, dst)| *dst = src as u8);

        Ok(Cow::Borrowed(buf.as_str()?))
    } else {
        Ok(Cow::Borrowed(constants::DEFAULT_CLIENT_ID))
    }
}

struct ArrayString<const N: usize>([u8; N]);

impl<const N: usize> Deref for ArrayString<{ N }> {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<const N: usize> DerefMut for ArrayString<{ N }> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<const N: usize> Display for ArrayString<{ N }> {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        for chunk in self.0.utf8_chunks() {
            f.write_str(chunk.valid())?;
        }

        Ok(())
    }
}

impl<const N: usize> ArrayString<{ N }> {
    const fn new() -> Self {
        Self([0u8; N])
    }

    fn random() -> Result<Self> {
        const ALPHANUMERIC: &[u8] = b"0123456789\
                                      ABCDEFGHIJKLMNOPQRSTUVWXYZ\
                                      abcdefghijklmnopqrstuvwxyz";

        let mut buf = [0u8; N];
        getrandom(&mut buf)?;

        for r in &mut buf {
            *r = ALPHANUMERIC[(*r as usize) % ALPHANUMERIC.len()];
        }

        Ok(Self(buf))
    }

    const fn as_str(&self) -> Result<&str, Utf8Error> {
        str::from_utf8(&self.0)
    }
}
