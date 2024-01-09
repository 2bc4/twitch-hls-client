## twitch-hls-client
Minimal CLI client for watching low latency Twitch streams

```
Usage: twitch-hls-client [OPTIONS] -p <PATH> <CHANNEL> <QUALITY>

Arguments:
  <CHANNEL>
          Twitch channel to watch (can also be twitch.tv/channel)
  <QUALITY>
          Stream quality/variant playlist to fetch (best, 1080p, 720p, 360p, 160p, audio_only, etc.)

Options:
  -s <URL>
          Playlist proxy server to fetch the master playlist from.
          If not specified will fetch the master playlist directly from Twitch.
          Can be multiple comma separated servers, will try each in order until successful.
          If URL includes "[channel]" it will be replaced with the channel argument at runtime.
  -p <PATH>
          Path to player
  -a <ARGUMENTS>
          Arguments to pass to the player
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
          Throw an error if a request is attempted without HTTPS
      --force-ipv4
          Only use IPv4 addresses when resolving host names
      --client-id <ID>
          Value to be used in the Client-Id header.
          If not specified will use the default client ID.
      --auth-token <TOKEN>
          Value to be used in the Authorization header.
          If --client-id is not specified will retrieve client ID from Twitch.
      --never-proxy <CHANNEL>
          Prevent specified channels from using a playlist proxy.
          Can be multiple comma separated channels.
      --http-retries <COUNT>
          Retry HTTP requests <COUNT> times before giving up [default: 3]
      --http-timeout <SECONDS>
          HTTP request timeout in seconds [default: 10]
  -h, --help
          Print help
  -V, --version
          Print version
```

### Example
```
$ twitch-hls-client twitch.tv/twitchchannel best -s https://eu.luminous.dev/live/[channel],https://lb-eu.cdn-perfprod.com/live/[channel] -p mpv -a '- --profile=low-latency'
[INFO] Opening player: mpv - --profile=low-latency
[INFO] Fetching playlist for channel twitchchannel (proxy)
[INFO] Using server https://eu.luminous.dev
 (+) Video --vid=1 (h264)
 (+) Audio --aid=1 (aac)
Using hardware decoding (vaapi).
VO: [gpu] 1920x1080 vaapi[nv12]
AO: [pipewire] 48000Hz stereo 2ch floatp
AV: 03:57:23 / 03:57:23 (100%) A-V:  0.000 Cache: 0.7s/482KB
```

### Building
Install [Rust](https://rustup.rs) then run `cargo install --locked --git https://github.com/2bc4/twitch-hls-client.git` or clone the repo and run `cargo build --release`.

#### Cargo features
- `http2` - Enable HTTP/2 support (known to cause issues on Windows 10)
- `static-curl` - Build and statically link to libcurl
- `static-openssl` - Build and statically link to OpenSSL (Only applies on platforms that use OpenSSL)
