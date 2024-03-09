{
  description = "A minimal command line client for watching/recording Twitch streams";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    systems.url = "github:nix-systems/default-linux";
  };

  outputs = {
    self,
    nixpkgs,
    fenix,
    systems,
    ...
  } @ inputs: let
    inherit (nixpkgs) lib;
    eachSystem = lib.genAttrs (import systems);
    pkgsFor = eachSystem (system:
      import nixpkgs {
        localSystem.system = system;
        overlays = [self.overlays.default];
      });
  in {
    overlays = {default = import ./nix/overlays.nix {inherit fenix;};};

    packages = eachSystem (system: {
      default = self.packages.${system}.twitch-hls-client;
      inherit (pkgsFor.${system}) twitch-hls-client;
    });

    homeManagerModules = {
      default = self.homeManagerModules.twitch-hls-client;
      twitch-hls-client = import ./nix/hm-module.nix self;
    };

    checks = eachSystem (system: self.packages.${system});

    formatter = eachSystem (system: pkgsFor.${system}.alejandra);
  };
}
