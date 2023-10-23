## twitch-hls-client
A very minimal (and buggy) low latency HLS client for Twitch

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
          If not specified will fetch master playlist from Twitch servers.
          Can be multiple comma separated servers, will try each in order until successful.
          If URL includes "[channel]" it will be replaced with the channel argument at runtime.
  -p <PATH>
          Path to the player that the stream will be piped to
  -a <ARGUMENTS>
          Arguments to pass to the player
  -c <PATH>
  	      Path to config file
  -d, --debug
          Enable debug logging
      --max-retries <COUNT>
          Attempt to fetch the media playlist <COUNT> times before exiting (default: 50)
      --passthrough
          Print the playlist URL to stdout and exit
      --client-id <ID>
          Value to be used in the Client-Id header.
	  Cannot be used if -s is set.
	  If not specified will use the default header.
      --auth-token <TOKEN>
          Value to be used in the Authorization header.
	  Cannot be used if -s is set.
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
First, install [Rust](https://rustup.rs).

Then run `cargo install --locked --git https://github.com/2bc4/twitch-hls-client.git` or clone the repo and run `cargo build --release`.
