pub const USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:144.0) Gecko/20100101 Firefox/144.0";

pub const PLAYER_VERSION: &str = "1.44.0-rc.1.1";

pub const TWITCH_GQL_ENDPOINT: &str = "https://gql.twitch.tv/gql";
pub const TWITCH_OAUTH_ENDPOINT: &str = "https://id.twitch.tv/oauth2/validate";
pub const TWITCH_HLS_BASE: &str = "https://usher.ttvnw.net/api/channel/hls/";

pub const KICK_CHANNELS_ENDPOINT: &str = "https://kick.com/api/v2/channels";

pub const DEFAULT_CLIENT_ID: &str = "kimne78kx3ncx6brgo4mv6wki5h1ko";
pub const DEFAULT_CONFIG_PATH: &str = concat!(env!("CARGO_PKG_NAME"), "/config");
