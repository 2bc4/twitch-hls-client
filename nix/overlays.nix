{fenix}: final: prev: {
  twitch-hls-client = prev.callPackage ./default.nix {
    lockFile = ./Cargo.lock;
    fenix = fenix;
  };
}
