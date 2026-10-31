# SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Nix flake for Pathfinder --- a jq 1.8.1-compatible jq on a fork of jaq's engine.
#
# Usage:
#   nix run . -- -r '.a' in.json   # run without installing (as `pathfinder`)
#   nix build .#pathfinder-jq      # build with the `jq` symlink; result/bin/jq
#   nix develop                    # development shell
#   nix flake check                # build + unit tests
#
# Two packages, on purpose:
#
#   packages.default / .pathfinder   the `pathfinder` binary only
#   packages.pathfinder-jq           the same, plus `bin/jq -> pathfinder`
#
# `packages.pathfinder-native` is `pathfinder-jq` under the name it had while
# the jaq engine was the package's default (until 0.2.0), kept for consumers
# that name it.
#
# Taking over the `jq` name is a choice the consumer makes explicitly; adding
# the default package to a profile never silently shadows a real jq.
#
# The package is defined once, in packaging/default.nix (Standard section 5.5).
# Built through this flake, its `../.` source is the git-tracked tree only, so
# untracked local state (target/, chat/) never reaches the store.
{
  description = "Pathfinder --- a jq 1.8.1-compatible jq on a fork of jaq's engine";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        pathfinder = pkgs.callPackage ./packaging/default.nix { };
        pathfinder-jq = pkgs.callPackage ./packaging/default.nix { withJqShim = true; };
        pathfinder-native = pathfinder-jq;
      in
      {
        packages = {
          default = pathfinder;
          inherit pathfinder pathfinder-jq pathfinder-native;
        };

        apps.default = {
          type = "app";
          program = "${pathfinder}/bin/pathfinder";
        };

        # buildRustPackage runs `cargo test` in its check phase. The
        # differential suite skips there (no real jq in the sandbox); CI runs it
        # against a pinned, checksum-verified jq 1.8.1.
        checks.default = pathfinder;

        devShells.default = pkgs.mkShell {
          name = "pathfinder-dev";
          packages = with pkgs; [
            cargo
            rustc
            clippy
            rustfmt
            reuse # section 4.3 gate
            texinfo # section 8 --- makeinfo doc/pathfinder.texi
            gnumake
          ];
          shellHook = ''
            echo "pathfinder dev shell."
            echo "  make check        fmt + clippy + tests"
            echo "  make diff         differential suite against real jq"
            echo "  reuse lint        section 4.3 gate"
          '';
        };
      }
    );
}
