## twitch-hls-client
`twitch-hls-client` is a minimal command line client for watching Twitch streams

### Features
- Playback of low latency and normal latency streams
- Ad blocking with playlist proxies or with a turbo/subscriber token
- Generally lower latency than the Twitch web player
- Minimal (uses 5-6MB of memory)

### Usage
Provide a player to output the stream to with `-p`, a channel to watch, and a stream quality.

Example:
```
$ twitch-hls-client -p mpv twitchchannel best
Fetching playlist for channel twitchchannel
Low latency streaming
Opening player: mpv -
 (+) Video --vid=1 (h264)
 (+) Audio --aid=1 (aac)
Using hardware decoding (vaapi).
VO: [gpu] 1920x1080 vaapi[nv12]
AO: [pipewire] 48000Hz stereo 2ch floatp
AV: 03:57:23 / 03:57:23 (100%) A-V:  0.000 Cache: 0.7s/482KB
```

That is the bare minimum, but there are many more options which can be viewed [here](src/usage) or by passing `--help`.

### Config file
Almost every option can also be set via config file. Example config file with all possible values set (values are made up):
```
# This is a comment
servers=https://eu.luminous.dev/live/[channel],https://lb-eu.cdn-perfprod.com/live/[channel]
player=../mpv/mpv
player-args=- --profile=low-latency
debug=true
quiet=true
passthrough=false
no-low-latency=false
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

|Platform   |Default location                                              |
|-----------|--------------------------------------------------------------|
|Linux & BSD|`${XDG_CONFIG_HOME:-${HOME}/.config}/twitch-hls-client/config`|
|Windows    |`%APPDATA%\twitch-hls-client\config`                          |
|MacOS      |`${HOME}/Library/Application Support/twitch-hls-client/config`|
|Other      |`./twitch-hls-client/config`                                  |

### Installing
There are standalone binaries built by GitHub for Linux and Windows [here](https://github.com/2bc4/twitch-hls-client/releases/latest).

Optionally, you can build it yourself by installing the [Rust toolchain](https://rustup.rs) and then running:
```
cargo install --locked --git https://github.com/2bc4/twitch-hls-client.git
```

#### Optional build time features
- `colors` - Enable terminal colors
- `http2` - Enable HTTP/2 support
- `debug-logging` - Enable debug logging support (disabling saves some CPU cycles and binary size)

### License
Distributed under the terms of the [GNU General Public License v3](https://www.gnu.org/licenses/gpl-3.0.txt), see [LICENSE](LICENSE) for more information.
