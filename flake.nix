{
  description = "smash: coding agent harness (terminal + desktop app)";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      each = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = each (pkgs: {
        default = pkgs.callPackage ./nix/package.nix { };
      });
      overlays.default = final: _prev: {
        smash = final.callPackage ./nix/package.nix { };
      };
      apps = each (pkgs: {
        default = {
          type = "app";
          program = "${self.packages.${pkgs.stdenv.hostPlatform.system}.default}/bin/smash";
        };
        desktop = {
          type = "app";
          program = "${self.packages.${pkgs.stdenv.hostPlatform.system}.default}/bin/smash-gpui";
        };
      });
    };
}
