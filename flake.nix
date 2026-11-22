{
  description = "nura-install: install Nura on a phone with an unlocked bootloader, without risking the bootloader";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      # The tool builds for Linux only: it relies on GNU coreutils' `df`.
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAll = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = forAll (pkgs: rec {
        nura-install = pkgs.callPackage ./pkgs/nura-install { };
        default = nura-install;
      });

      overlays.default = final: _prev: {
        nura-install = final.callPackage ./pkgs/nura-install { };
      };

      checks = forAll (
        pkgs:
        let
          hooked = pkgs.callPackage ./pkgs/nura-install { testHooks = true; };
          # The shipped build with a cache the sandbox can write, so its "nothing to do" path can run.
          shippedCacheDir = "/tmp/nura-install-shipped-cache";
          shipped = pkgs.callPackage ./pkgs/nura-install { cacheDir = shippedCacheDir; };
        in
        {
          nura-install =
            pkgs.runCommand "nura-install-test"
              {
                nativeBuildInputs = with pkgs; [
                  util-linux
                ];
              }
              ''
                bash ${./tests/nura-install-test.sh} ${pkgs.lib.getExe hooked} ${pkgs.lib.getExe shipped} ${shippedCacheDir}
                touch $out
              '';
        }
      );

      devShells = forAll (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [
            cargo
            clippy
            rustc
            rustfmt
            gcc
          ];
        };
      });

      formatter = forAll (pkgs: pkgs.nixfmt-rfc-style);
    };
}
