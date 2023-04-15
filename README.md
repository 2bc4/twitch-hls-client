## twitch-hls-client
Lightweight CLI client for watching low latency Twitch streams with ad blocking.

Fetches the playlist from the ad blocking playlist proxy server of your choosing, fetches the latest prefetch segment, and then writes the MPEG-TS stream to your player.

```
Usage: twitch-hls-client [OPTIONS] --server <URL> <CHANNEL> <QUALITY>

Arguments:
  <CHANNEL>
          Twitch channel to watch (can also be twitch.tv/channel for Streamlink compatibility)
  <QUALITY>
          Stream quality/variant playlist to fetch (best, 1080p, 720p, 360p, 160p, audio_only)

Options:
  -s, --server <URL>
          Playlist proxy server to fetch the playlist from.
          Can be multiple comma separated servers, will try each in order until successful.
          If URL path is "[ttvlol]" the playlist will be requested using the TTVLOL API.
          If URL includes "[channel]" it will be replaced with the channel argument at runtime.
  -p, --player <PATH>
          Path to the player that the stream will be piped to, if not specified will write stream to stdout
  -a, --player-args <ARGUMENTS>
          Arguments to pass to the player
      --player-buffer-size <SIZE>
          Player write buffer size in bytes [default: 4096]
      --disable-reset-on-ad
          Disables resetting the player and stream when encountering an embedded advertisement
  -d, --debug
          Enable debug logging
  -h, --help
          Print help
  -V, --version
          Print version
```

### Example
```sh
$ twitch-hls-client twitch.tv/twitchchannel best -s https://api.ttv.lol/[ttvlol],https://eu.luminous.dev/live/[channel] -p /usr/bin/mpv -a '- --profile=low-latency --demuxer-max-bytes=700k'
[INFO] Opening player: mpv - --profile=low-latency --demuxer-max-bytes=700k
[INFO] Fetching playlist for channel twitchchannel
[INFO] Using server https://api.ttv.lol
[INFO] Trying TTVLOL API
[INFO] Catching up to latest segment
 (+) Video --vid=1 (h264)
 (+) Audio --aid=1 (aac)
Using hardware decoding (vaapi).
VO: [gpu] 1920x1080 vaapi[nv12]
AO: [pipewire] 48000Hz stereo 2ch floatp
AV: 03:57:23 / 03:57:23 (100%) A-V:  0.000 Cache: 0.7s/482KB
```
