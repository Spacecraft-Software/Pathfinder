<!--
SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
SPDX-License-Identifier: CC-BY-SA-4.0
-->

# Pathfinder — Upstream

Where the engine crates under `crates/` came from (PLAN P-049). Pathfinder's
engine is a hard fork of [jaq](https://github.com/01mf02/jaq) by Michael
Färber and contributors, MIT-licensed. Nothing here is sent upstream
(Standard §6.4); fixes from later jaq releases are ported by hand (TODO T-020).

## Import base

| What | Value |
|---|---|
| jaq version | **3.1.1** (the `jaq` CLI crate; library crates at their own versions, below) |
| Upstream tag | `v3.1.1` (lightweight) |
| Upstream commit | `c866e70303b5dbc37d83a0b0cbacf10e90af9c8c` (`git ls-remote https://github.com/01mf02/jaq refs/tags/v3.1.1`) |
| Taken from | nixpkgs `jaq.src` at this flake's locked nixpkgs |
| nixpkgs rev | `151fa4e8ddfdd8dd25d945ad94ed54a13de9f6e4` (`narHash` `sha256-Miqqk/ammqnTUxaoCyvtwoPeLbI1ksFyLxi/CazZpWY=`, `lastModified` 1791260994) |
| Source URL | `https://github.com/01mf02/jaq/archive/refs/tags/v3.1.1.tar.gz` |
| Source hash | `sha256-/yAwLcPwfW5UH+PCCrsFaM0Nuk1S5QONLsNgvVCBLX8=` (`jaq.src.outputHash`) |
| Store path | `/nix/store/i9ws7pnvawh78xc8mx44mk36nxgk07m9-source` |

To reproduce the source tree:

```sh
nix eval --raw --inputs-from . nixpkgs#jaq.version                 # 3.1.1
nix build --no-link --print-out-paths --inputs-from . nixpkgs#jaq.src
```

The tree the patches in `packaging/jaq/` were developed against was checked
with `diff -r` and is identical to that store path.

## What was imported

| Crate | From | Upstream version |
|---|---|---|
| `crates/pathfinder-core` | `jaq-core/` (without `fuzz/`, a separate Cargo root; fuzzing returns in T-019) | 3.1.0 |
| `crates/pathfinder-std` | `jaq-std/` | 3.0.1 |
| `crates/pathfinder-json` | `jaq-json/` | 2.0.2 |
| `crates/pathfinder-fmts` | `jaq-fmts/`, whole | 0.1.1 |

`jaq-all`, the `jaq` CLI crate and `jaq-play` were not imported.

`jaq-fmts` is its own crate rather than part of `pathfinder-json`: the
functions jaq's CLI reads and writes with (`read::read`, `read::parse`,
`read::read_string`, `read::bytes_str`, `write::write`) live in
`read/formats.rs` and `write/formats.rs`, behind the `all` feature, and
dispatch over every format at once. There is no JSON-and-raw subset to copy
without editing those files, so it is copied whole and stays diffable against
upstream; the YAML, TOML, CBOR, XML and CSV/TSV code and their dependencies go
in T-004.

The only differences from upstream are in the manifests:

- `[package] name` is `pathfinder-*`; `readme` points at Pathfinder's README.
- `[lib] name` keeps the upstream crate name (`jaq_core`, `jaq_std`,
  `jaq_json`, `jaq_fmts`), and the workspace's `[workspace.dependencies]`
  maps the keys `jaq-core`, … to the `pathfinder-*` packages. Every `.rs` and
  `.jq` file therefore compiles as upstream wrote it and stays byte-identical
  to it, which keeps the first diff against upstream reviewable.
- The dev-dependencies on sibling crates name the `pathfinder-*` packages.
- `pathfinder-std` and `pathfinder-fmts` gain `categories`, which upstream
  leaves out and the workspace's `clippy::cargo` lints demand.

Dependency versions are jaq's `Cargo.lock` at the import base, except where
the shim already locked a newer semver-compatible version and Cargo unified
the two: `bstr` 1.13.1 (jaq 1.13.0), `serde`/`serde_core`/`serde_derive`
1.0.229 (1.0.228), `serde_json` 1.0.151 (1.0.149), `proc-macro2` 1.0.107
(1.0.106), `quote` 1.0.47 (1.0.41), `unicode-ident` 1.0.24 (1.0.18). No
dependency is new; none comes from git.
