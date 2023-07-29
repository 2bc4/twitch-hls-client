Archived for the forseeable future due to demotivation and lack of time. Was a fun first Rust project though.

## twitch-hls-client
Lightweight CLI client for watching low latency Twitch streams with ad blocking.

Fetches the playlist from the ad blocking playlist proxy server of your choosing, fetches the latest prefetch segment, and then writes the MPEG-TS stream to your player.

```
Usage: twitch-hls-client [OPTIONS] -s <URL> -p <PATH> <CHANNEL> <QUALITY>

Arguments:
  <CHANNEL>
          Twitch channel to watch (can also be twitch.tv/channel)
  <QUALITY>
          Stream quality/variant playlist to fetch (best, 1080p, 720p, 360p, 160p, audio_only, etc.)

Options:
  -s <URL>
          Playlist proxy server to fetch the master playlist from.
          Can be multiple comma separated servers, will try each in order until successful.
          If URL path is "[ttvlol]" the playlist will be requested using the TTVLOL API.
          If URL includes "[channel]" it will be replaced with the channel argument at runtime.
  -p <PATH>
          Path to the player that the stream will be piped to
  -a <ARGUMENTS>
          Arguments to pass to the player
  -d, --debug
          Enable debug logging
      --max-retries <COUNT>
          Attempt to fetch the media playlist <COUNT> times before exiting (default: 30)
      --passthrough
          Print the playlist URL to stdout and exit
  -h, --help
          Print help
  -V, --version
          Print version
```

### Example
```
$ twitch-hls-client twitch.tv/twitchchannel best -s https://api.ttv.lol/[ttvlol],https://eu.luminous.dev/live/[channel] -p mpv -a '- --profile=low-latency'
[INFO] Opening player: mpv - --profile=low-latency --force-media-title=twitch.tv/twitchchannel
[INFO] Fetching playlist for channel twitchchannel
[INFO] Using server https://api.ttv.lol (TTVLOL API)
 (+) Video --vid=1 (h264)
 (+) Audio --aid=1 (aac)
Using hardware decoding (vaapi).
VO: [gpu] 1920x1080 vaapi[nv12]
AO: [pipewire] 48000Hz stereo 2ch floatp
AV: 03:57:23 / 03:57:23 (100%) A-V:  0.000 Cache: 0.7s/482KB
```

### Installing prebuilt binaries
Head to [releases](https://github.com/2bc4/twitch-hls-client/releases/latest) and download the .zip file for your platform and then extract it. The twitch-hls-client(.exe) binary is self-contained and can be moved anywhere.

Binaries are built by [GitHub CI](https://github.com/2bc4/twitch-hls-client/actions/workflows/release.yaml) from this repo, the sha512sums file is then generated locally and signed with my [public key](https://github.com/2bc4/2bc4).

### Building
First, install [Rust](https://rustup.rs).

If you wish to build from the latest commit run `cargo install --locked --git https://github.com/2bc4/twitch-hls-client.git` or clone the repo and run `cargo build --release`.

If you just want to build the latest release it's on [crates.io](https://crates.io/crates/twitch-hls-client). Run `cargo install twitch-hls-client`.
