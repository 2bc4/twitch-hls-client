## twitch-hls-client
[![crates.io](https://img.shields.io/crates/v/twitch-hls-client.svg)](https://crates.io/crates/twitch-hls-client)
[![nixpkgs unstable](https://repology.org/badge/version-for-repo/nix_unstable/twitch-hls-client.svg?header=nixpkgs%20unstable)](https://search.nixos.org/packages?channel=unstable&show=twitch-hls-client)
[![license](https://img.shields.io/github/license/2bc4/twitch-hls-client.svg)](LICENSE)

`twitch-hls-client` is a minimal command line client for watching/recording Twitch streams

### Features
- Playback of low latency and normal latency streams
- Ad blocking with playlist proxies or with a turbo/subscriber token
- Generally lower latency than the Twitch web player
- Tiny (at most uses 3-4MB of memory depending on platform)

### Usage
#### Watching
Provide a player to output the stream to with `-p`, a channel, and a stream quality.

Example:
```
$ twitch-hls-client -p mpv twitchchannel best
Fetching playlist for channel twitchchannel
Opening player: mpv -
[file] Reading from stdin...
 (+) Video --vid=1 (h264)
 (+) Audio --aid=1 (aac)
Using hardware decoding (vaapi).
VO: [dmabuf-wayland] 1920x1080 vaapi[nv12]
AO: [pipewire] 48000Hz stereo 2ch floatp
AV: 03:57:23 / 03:57:23 (100%) A-V:  0.000
```

#### Recording
Provide a file path to output the stream to with `-r`, a channel, and a stream quality.

Example:
```
$ twitch-hls-client -r recording.ts twitchchannel best
Fetching playlist for channel twitchchannel
Recording to: recording.ts
```

The recorded file will be in raw MPEG-TS format.
While you can view it directly, you may want to remux it to a more suitable container.
For example with ffmpeg:
```
$ ffmpeg -i recording.ts -codec copy recording.mkv
```

#### TCP server
Provide an address and port to listen on with `-t`, a channel, and a stream quality:

Example:
```
$ twitch-hls-client -t 127.0.0.1:8080 twitchchannel best
Fetching playlist for channel twitchchannel
Listening on: 127.0.0.1:8080
Waiting for outputs...
```

Example watching with mpv:
```
$ mpv tcp://127.0.0.1:8080
```

----------

You can also use any combination of outputs at the same time.<br/>
That is the bare minimum, but there are many more options which can be viewed [here](src/usage) or by passing `--help`.


### Using turbo/subscriber token
With `--auth-token` you can set an OAuth token to be used when fetching the master playlist. Twitch won't serve ads if the Twitch account associated with the OAuth token is subscribed to turbo or the specific channel you're watching.

To find your token, navigate to the Twitch website in a browser and open your browser's devtools (press F12). Click the `Application` (Chrome) or `Storage` (Firefox) tab and find `https://www.twitch.tv` under `Cookies` on the left. Your token is the string of characters beside the cookie named `auth-token`.

### Ad blocking
You can use `-s` to request the master playlist from a proxy server, which bypasses ads by either using a turbo token or requesting the playlist from a country where Twitch doesn't serve ads. A list of known public servers can be found [here](https://github.com/2bc4/twitch-hls-client/wiki/Known-public-playlist-proxy-servers).

### Config file
Almost every option can also be set via config file. There is an example config file with all possible values set [here](example-config).

Depending on your platform this will look for the config file at the following locations (can be overridden with `-c`):

|Platform   |Default location                                              |
|-----------|--------------------------------------------------------------|
|Linux & BSD|`${XDG_CONFIG_HOME:-${HOME}/.config}/twitch-hls-client/config`|
|Windows    |`%APPDATA%\twitch-hls-client\config`                          |
|MacOS      |`${HOME}/Library/Application Support/twitch-hls-client/config`|
|Other      |`./twitch-hls-client/config`                                  |

### Installing
There are standalone binaries built by GitHub for Linux and Windows [here](https://github.com/2bc4/twitch-hls-client/releases/latest).


### Building
Install the [Rust toolchain](https://rustup.rs) and then run:
```
cargo install --git https://github.com/2bc4/twitch-hls-client.git
```
Or from [crates.io](https://crates.io/crates/twitch-hls-client):
```
cargo install twitch-hls-client
```

Building requires a C/C++ compiler for ring (TLS cryptographic primitives). You should prefer clang over gcc due to a gcc bug resulting in worse TLS performance.

#### NixOS

<details closed>
<summary>Flake Package</summary>

```nix
# flake.nix

{
  inputs.twitch-hls-client.url = "github:2bc4/twitch-hls-client";
  # ...

  outputs = {nixpkgs, ...} @ inputs: {
    nixosConfigurations.HOSTNAME = nixpkgs.lib.nixosSystem {
      specialArgs = { inherit inputs; }; # this is the important part
      modules = [
        ./configuration.nix
      ];
    };
  } 
}
```

```nix
# configuration.nix

{inputs, pkgs, ...}: {
  programs.twitch-hls-client = {
    enable = true;
    package = inputs.twitch-hls-client.packages.${pkgs.system}.default;
  };
}
```

</details>

<details closed>
<summary>Flake Home-Manager</summary>

```nix
# twitch-hls-client.nix
{
  programs.twitch-hls-client = {
    enable = true;
    # ...

    # This is a example to use -c config file every time
    systemd.user.services.twitch-hls-client = {
      Unit = {
        Description = "Twitch HLS Client Service";
      };

      Service = {
        Type = "simple";
        ExecStart = "twitch-hls-client -c ${config.xdg.configHome}/twitch-hls-client/config";
        Restart = "always";
      };

      Install = {
        WantedBy = ["default.target"];
      };
    };
  };
}
```

</details>

### Reducing player latency with mpv
Following [this section](https://mpv.io/manual/master/#low-latency-playback) in the mpv manual is recommended.

### License
Distributed under the terms of the [GNU General Public License v3](https://www.gnu.org/licenses/gpl-3.0.txt), see [LICENSE](LICENSE) for more information.
