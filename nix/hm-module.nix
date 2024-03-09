self: {
  config,
  pkgs,
  lib,
  ...
}: let
  inherit (lib.types) bool int package str;
  inherit (lib.modules) mkIf;
  inherit (lib.options) mkOption mkEnableOption;

  boolToString = x:
    if x
    then "true"
    else "false";
  cfg = config.programs.twitch-hls-client;
  filterOptions = options:
    builtins.filter (opt: builtins.elemAt opt 1 != "") options;
in {
  options.programs.twitch-hls-client = {
    enable =
      mkEnableOption ""
      // {
        description = ''
          twitch-hls-client is a minimal command line client for watching/recording Twitch streams

          Features
          Playback of low latency and normal latency streams
          Ad blocking with playlist proxies or with a turbo/subscriber token
          Generally lower latency than the Twitch web player
          Tiny (at most uses 3-4MB of memory)
        '';
      };

    package = mkOption {
      description = "The twitch-hls-client package";
      type = package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.twitch-hls-client;
    };
    player = mkOption {
      description = "Path to player";
      type = package;
      default = pkgs.mpv;
    };
    player-args = mkOption {
      description = "Arguments to pass to the player [default: -]";
      type = str;
    };
    record = mkOption {
      description = "Record to <PATH>";
      type = str;
    };
    servers = mkOption {
      description = "Ad blocking playlist proxy server to fetch the master playlist from. If not specified will fetch the master playlist directly from Twitch. Can be multiple comma separated servers, will try each in order until successful. If URL includes the keyword '[channel]' it will be replaced with the channel argument at runtime. Note: This does not support standard HTTP proxies (ie. proxies using the CONNECT request)";
      type = str;
    };
    debug = mkOption {
      description = "Enable debug logging";
      type = bool;
      default = false;
    };
    quiet = mkOption {
      description = "Silence player output";
      type = bool;
      default = true;
    };
    passthrough = mkOption {
      description = "Passthrough playlist URL to player and do nothing else";
      type = bool;
    };
    print-streams = mkOption {
      description = "Print available stream qualities and exit";
      type = bool;
    };
    overwrite = mkOption {
      description = "Allow overwriting file when recording";
      type = bool;
    };
    no-low-latency = mkOption {
      description = "Disable low latency streaming";
      type = bool;
    };
    no-kill = mkOption {
      description = "Don't kill the player on exit";
      type = bool;
    };
    force-https = mkOption {
      description = "Abort request if protocol is not HTTPS";
      type = bool;
      default = true;
    };
    force-ipv4 = mkOption {
      description = "Only use IPv4 addresses when resolving host names";
      type = bool;
      default = false;
    };
    client-id = mkOption {
      description = "Value to be used in the Client-Id header. If not specified will use the default client ID";
      type = str;
    };
    auth-token = mkOption {
      description = "Value to be used in the Authorization header. If --client-id is not specified will retrieve client ID from Twitch";
      type = str;
    };
    never-proxy = mkOption {
      description = "Prevent specified channels from using a playlist proxy. Can be multiple comma separated channels";
      type = str;
    };
    codecs = mkOption {
      description = "Comma separated list of supported codecs [default: av1,h265,h264]";
      type = str;
    };
    user-agent = mkOption {
      description = "User agent used in HTTP requests [default: a recent version of Firefox on Windows 10]";
      type = str;
    };
    http-retries = mkOption {
      description = "Retry HTTP requests <COUNT> times before giving up [default: 3]";
      type = int;
    };
    http-timeout = mkOption {
      description = "HTTP request timeout in seconds [default: 10]";
      type = int;
    };
    quality = mkOption {
      description = "Stream quality/variant playlist to stream (best, 1080p, 720p, 360p, 160p, audio_only, etc.)";
      type = str;
      default = "720p";
    };
  };

  config = mkIf cfg.enable {
    home.packages = [cfg.package];

    xdg.configFile."twitch-hls-client/config".text = let
      formatOption = name: value: "${name}=${value}";
      formatConfig = options:
        builtins.concatStringsSep "\n" (map (opt: formatOption (builtins.head opt) (builtins.elemAt opt 1)) options);
    in ''
      ${formatConfig (filterOptions [
        ["player" (lib.getExe cfg.player)]
        ["debug" (boolToString cfg.debug)]
        ["quiet" (boolToString cfg.quiet)]
        ["force-https" (boolToString cfg."force-https")]
        ["force-ipv4" (boolToString cfg."force-ipv4")]
        ["quality" cfg.quality]
      ])}
    '';
  };
}
