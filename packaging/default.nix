# SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
# SPDX-License-Identifier: GPL-3.0-or-later
{
  lib,
  rustPlatform,
  jaq,
  makeWrapper,
  # Install the `jq` symlink alongside the binary. Off by default so the package
  # can be added to a profile without silently taking over the `jq` name; the
  # flake exposes both variants.
  withJqShim ? false,
}:

rustPlatform.buildRustPackage rec {
  pname = "pathfinder";
  version = "0.1.0";

  src = lib.cleanSource ../.;
  cargoLock.lockFile = ../Cargo.lock;

  nativeBuildInputs = [ makeWrapper ];

  # The differential suite needs a real jq, which is not a build input here.
  # It skips cleanly when jq is absent; CI is where it actually runs.
  doCheck = true;

  postInstall = ''
    # Pin jaq by store path rather than trusting the user's PATH: the shim is
    # useless without it, and a PATH miss would surface as a confusing failure
    # inside something that looks like jq.
    wrapProgram "$out/bin/pathfinder" \
      --set-default PATHFINDER_JAQ "${jaq}/bin/jaq"
  '' + lib.optionalString withJqShim ''
    ln -s pathfinder "$out/bin/jq"
  '';

  meta = with lib; {
    description = "jq-compatible shim over jaq";
    homepage = "https://Pathfinder.SpacecraftSoftware.org/";
    license = licenses.gpl3Plus;
    maintainers = [ ];
    mainProgram = "pathfinder";
    platforms = platforms.unix;
  };
}
