{
  description = "nura-install: install Nura on a phone with an unlocked bootloader, without risking the bootloader";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      # adb and fastboot reach the phone through udev-granted USB access, and the partition copy
      # reads Linux's network state, so the tool builds for Linux only.
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
                  curl
                  jq
                  util-linux
                  xz
                ];
              }
              ''
                FP6_GETVAR_ALL=${./tests/fixtures/fp6-getvar-all.txt} NURA_FIXTURE=${./tests/fixtures/nura-index.json} \
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
