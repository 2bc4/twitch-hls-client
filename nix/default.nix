{
  system,
  pkgs,
  lib,
  makeWrapper,
  rustPlatform,
  pkg-config,
  rustfmt,
  lockFile,
  fenix,
}: let
  cargoToml = builtins.fromTOML (builtins.readFile ../Cargo.toml);
  toolchain = fenix.packages.${system}.minimal.toolchain;
in
  (pkgs.makeRustPlatform {
    cargo = toolchain;
    rustc = toolchain;
  })
  .buildRustPackage rec {
    pname = cargoToml.package.name;
    version = cargoToml.package.version;

    src = ../.;

    cargoLock = {
      lockFile = ../Cargo.lock;
    };

    nativeBuildInputs = [
      pkg-config
      makeWrapper
      rustfmt
    ];

    doCheck = true;
    CARGO_BUILD_INCREMENTAL = "false";
    RUST_BACKTRACE = "full";
    copyLibs = true;

    postInstall = ''
      wrapProgram $out/bin/twitch-hls-client
    '';

    meta = {
      homepage = "https://github.com/2bc4/twitch-hls-client";
      description = "A minimal command line client for watching/recording Twitch streams";
      license = lib.licenses.gpl3;
      platforms = lib.platforms.all;
      mainProgram = "twitch-hls-client";
    };
  }
