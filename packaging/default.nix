# SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
# SPDX-License-Identifier: GPL-3.0-or-later
{
  lib,
  rustPlatform,
  # Install the `jq` symlink alongside the binary. Off by default so the package
  # can be added to a profile without silently taking over the `jq` name; the
  # flake exposes both variants.
  withJqShim ? false,
  # Install `mjq`, a short name for Pathfinder itself: it answers as
  # `pathfinder` does (its own verbs included), and shadows nothing.
  withMjqAlias ? true,
}:

rustPlatform.buildRustPackage rec {
  pname = "pathfinder";
  version = "0.1.0";

  src = lib.cleanSource ../.;
  cargoLock.lockFile = ../Cargo.lock;

  # The differential suite needs a real jq 1.8.1, which is not a build input
  # here. It skips cleanly when none is found; CI is where it actually runs.
  doCheck = true;

  postInstall =
    lib.optionalString withJqShim ''
      ln -s pathfinder "$out/bin/jq"
    ''
    + lib.optionalString withMjqAlias ''
      ln -s pathfinder "$out/bin/mjq"
    '';

  meta = with lib; {
    description = "jq 1.8.1-compatible jq, with its own engine";
    homepage = "https://Pathfinder.SpacecraftSoftware.org/";
    license = licenses.gpl3Plus;
    maintainers = [ ];
    mainProgram = "pathfinder";
    platforms = platforms.unix;
  };
}
