## twitch-hls-client
[![crates.io](https://img.shields.io/crates/v/twitch-hls-client.svg)](https://crates.io/crates/twitch-hls-client)
[![license](https://img.shields.io/github/license/2bc4/twitch-hls-client.svg)](LICENSE)

Minimal CLI client for watching low latency Twitch streams

```
Usage: twitch-hls-client [OPTIONS] -p <PATH> <CHANNEL> <QUALITY>

Arguments:
  <CHANNEL>
          Twitch channel to watch (can also be twitch.tv/channel)
  <QUALITY>
          Stream quality/variant playlist to fetch (best, 1080p, 720p, 360p, 160p, audio_only, etc.)

Options:
  -s <URL1,URL2>
          Ad blocking playlist proxy server to fetch the master playlist from.
          If not specified will fetch the master playlist directly from Twitch.
          Can be multiple comma separated servers, will try each in order until successful.
          If URL includes "[channel]" it will be replaced with the channel argument at runtime.
  -p <PATH>
          Path to player
  -a <ARGUMENTS>
          Arguments to pass to the player [default: -]
  -c <PATH>
          Path to config file
  -d, --debug
          Enable debug logging
  -q, --quiet
          Silence player output
      --passthrough
          Passthrough playlist URL to player and do nothing else
      --no-config
          Ignore config file
      --no-kill
          Don't kill the player on exit
      --force-https
          Abort request if protocol is not HTTPS
      --force-ipv4
          Only use IPv4 addresses when resolving host names
      --client-id <ID>
          Value to be used in the Client-Id header.
          If not specified will use the default client ID.
      --auth-token <TOKEN>
          Value to be used in the Authorization header.
          If --client-id is not specified will retrieve client ID from Twitch.
      --never-proxy <CHANNEL1,CHANNEL2>
          Prevent specified channels from using a playlist proxy.
          Can be multiple comma separated channels.
      --codecs <CODEC1,CODEC2>
          Comma separated list of supported codecs [default: av1,h265,h264]
      --user-agent <USERAGENT>
          Set user agent used in HTTP requests [default: a recent version of Firefox]
      --http-retries <COUNT>
          Retry HTTP requests <COUNT> times before giving up [default: 3]
      --http-timeout <SECONDS>
          HTTP request timeout in seconds [default: 10]
  -h, --help
          Print help
  -V, --version
          Print version
```

### Example usage
```
$ twitch-hls-client -p mpv -a '- --profile=low-latency' twitchchannel best
Fetching playlist for channel twitchchannel
Opening player: mpv - --profile=low-latency
 (+) Video --vid=1 (h264)
 (+) Audio --aid=1 (aac)
Using hardware decoding (vaapi).
VO: [gpu] 1920x1080 vaapi[nv12]
AO: [pipewire] 48000Hz stereo 2ch floatp
AV: 03:57:23 / 03:57:23 (100%) A-V:  0.000 Cache: 0.7s/482KB
```

### Config file
Almost every argument can be set via config file. Example config file with all possible values set:
```
# This is a comment
servers=https://eu.luminous.dev/live/[channel],https://lb-eu.cdn-perfprod.com/live/[channel]
player=../mpv/mpv
player-args=- --profile=low-latency
debug=true
quiet=true
passthrough=false
no-kill=false
force-https=true
force-ipv4=false
client-id=0123456789abcdef
auth-token=0123456789abcdef
never-proxy=channel1,channel2,channel3
codecs=av1,h265,h264
user-agent=Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:122.0) Gecko/20100101 Firefox/122.0
http-retries=3
http-timeout=10
quality=720p
```

Depending on your platform this will look for the config file at the following locations (can be overridden with `-c`):

|Platform|Default location                                              |
|--------|--------------------------------------------------------------|
|Windows |`%APPDATA%\twitch-hls-client\config`                          |
|Linux   |`${XDG_CONFIG_HOME:-${HOME}/.config}/twitch-hls-client/config`|
|MacOS   |`${HOME}/Library/Application Support/twitch-hls-client/config`|
|Other   |`./twitch-hls-client/config`                                  |

### Building
Install [Rust](https://rustup.rs) then run `cargo install --locked --git https://github.com/2bc4/twitch-hls-client.git` or clone the repo and run `cargo build --release`.

#### Cargo features
- `colors` - Enable terminal colors
- `http2` - Enable HTTP/2 support (known to cause issues on Windows 10)
- `static-curl` - Build and statically link to libcurl
- `static-openssl` - Build and statically link to openssl (only applies on platforms that use openssl)
- `rustls` - Use rustls instead of the system tls lib (has no effect if dynamic linking to libcurl)
