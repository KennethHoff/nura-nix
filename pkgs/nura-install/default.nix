# nura-install: guarded Nura install for phones with an unlocked bootloader (the Fairphone (Gen. 6) is the one tested).
#
# ## Why it shells out
#
# It runs adb, fastboot, curl, jq, tar, xz, ssh and ip instead of
# linking crates for USB, HTTP, JSON, LZMA, SSH and netlink, so the only
# crate is libc, for the effective user id, the umask, holding signals during a
# group of writes and reading the terminal without buffering. The wrapper puts
# those tools on PATH.
#
# ## Index, cache and test hooks
#
# Nura's builds come from its image index, fetched on every run and checked file by file against
# the hashes in it. `cacheDir` is compiled in
# as `NURA_INSTALL_CACHE` when set (otherwise the cache is $XDG_DATA_HOME/nura-install), so an
# exported NURA_INSTALL_* variable changes nothing in the shipped binary.
# With `testHooks = true` the crate's `test-hooks` feature also reads
# NURA_INSTALL_* from the environment, which lets the flake check stand in for the
# phone, the image servers, the SSH tools and the terminal. Only the flake check
# builds that.
{
  lib,
  rustPlatform,
  makeWrapper,
  android-tools,
  coreutils,
  curl,
  gnutar,
  iproute2,
  jq,
  openssh,
  xz,
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
  nativeCheckInputs = [ jq ];

  postInstall = ''
    wrapProgram $out/bin/nura-install \
      --prefix PATH : ${
        lib.makeBinPath [
          android-tools
          coreutils
          curl
          gnutar
          iproute2
          jq
          openssh
          xz
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
