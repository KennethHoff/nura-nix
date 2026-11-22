# nura-install: guarded Nura install for phones with an unlocked bootloader (the Fairphone (Gen. 6) is the one tested).
#
# ## Why it shells out
#
# It runs adb and fastboot instead of linking a crate for USB, so the only
# crate is libc, for the effective user id, the umask and catching Ctrl-C. The wrapper puts
# those tools on PATH.
#
# ## Cache and test hooks
#
# `cacheDir` is compiled in
# as `NURA_INSTALL_CACHE` when set (otherwise the cache is $XDG_DATA_HOME/nura-install), so an
# exported NURA_INSTALL_* variable changes nothing in the shipped binary.
# With `testHooks = true` the crate's `test-hooks` feature also reads
# NURA_INSTALL_* from the environment, which lets the flake check stand in for the
# phone. Only the flake check
# builds that.
{
  lib,
  rustPlatform,
  makeWrapper,
  android-tools,
  coreutils,
  cacheDir ? null,
  testHooks ? false,
}:
rustPlatform.buildRustPackage {
  pname = "nura-install";
  version = "0.3.0";

  src = lib.fileset.toSource {
    root = ./.;
    fileset = lib.fileset.unions [
      ./Cargo.toml
      ./Cargo.lock
      ./src
    ];
  };

  cargoLock.lockFile = ./Cargo.lock;

  buildFeatures = lib.optional testHooks "test-hooks";

  env = lib.optionalAttrs (cacheDir != null) { NURA_INSTALL_CACHE = cacheDir; };

  nativeBuildInputs = [ makeWrapper ];

  postInstall = ''
    wrapProgram $out/bin/nura-install \
      --prefix PATH : ${
        lib.makeBinPath [
          android-tools
          coreutils
        ]
      }
  '';

  meta = {
    description = "Guarded Nura install for phones with an unlocked bootloader";
    license = lib.licenses.mit;
    mainProgram = "nura-install";
    platforms = lib.platforms.linux;
  };
}
