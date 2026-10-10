# SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
# SPDX-License-Identifier: GPL-3.0-or-later
{
  lib,
  rustPlatform,
  jaq,
  # Install the `jq` symlink alongside the binary. Off by default so the package
  # can be added to a profile without silently taking over the `jq` name; the
  # flake exposes both variants.
  withJqShim ? false,
  # Apply Pathfinder's patch set (packaging/jaq/) to the pinned jaq, so it
  # prints and reads numbers, and repeats strings, as jq 1.8.1 does. The
  # patches are carried here and never sent upstream (Standard sections 4.2,
  # 6.4). Pathfinder works on a stock jaq too; this only raises fidelity.
  withPatchedJaq ? true,
}:

let
  engine =
    if withPatchedJaq then
      jaq.overrideAttrs (old: {
        pname = "jaq-pathfinder";
        patches = (old.patches or [ ]) ++ [
          ./jaq/0001-version-marker.patch
          ./jaq/0002-number-output.patch
          ./jaq/0003-arithmetic-precision.patch
          ./jaq/0004-input-literals.patch
          ./jaq/0005-string-repeat.patch
        ];
        # jaq's own tests assert jaq's number format (`1.0`, `NaN`), which the
        # patches change on purpose. Pathfinder's check phase runs jq's suite
        # against this engine instead (tests/jq-suite/FLOOR-patched).
        doCheck = false;
      })
    else
      jaq;
in
rustPlatform.buildRustPackage rec {
  pname = "pathfinder";
  version = "0.1.0";

  src = lib.cleanSource ../.;
  cargoLock.lockFile = ../Cargo.lock;

  # Pin jaq by store path rather than trusting the user's PATH: the shim is
  # useless without it, and a PATH miss would surface as a confusing failure
  # inside something that looks like jq. The path is compiled in (see
  # `PINNED_JAQ` in src/main.rs) instead of set by `wrapProgram`, because a
  # wrapper script would fork a shell before every `jq` call. PATHFINDER_JAQ
  # still overrides it at run time.
  env.PATHFINDER_DEFAULT_JAQ = "${engine}/bin/jaq";

  # The differential suite needs a real jq 1.8.1, which is not a build input
  # here. It skips cleanly when none is found; CI is where it actually runs.
  doCheck = true;

  postInstall = lib.optionalString withJqShim ''
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
