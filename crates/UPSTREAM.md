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
| `crates/pathfinder-jaq` | `jaq-all/src/` as its library, `jaq/src/` as its binary (T-010, below) | 0.3.0 / 3.1.1 |

`jaq-play`, `jaq-all`'s examples and the `jaq` crate's `tests/` (golden tests
of the CLI, mostly of the formats and `--in-place` that M18 removes) were not
imported.

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

## Patches applied

`packaging/jaq/0001`–`0015`, the 3.1.1 cuts (never the `-3.1.0` ones), were
applied in order with `git apply`, their `jaq-core/`, `jaq-std/`, `jaq-json/`
and `jaq-fmts/` paths rewritten to `crates/pathfinder-*/`. Every hunk applied
cleanly. The result was checked with `diff -r` against the tree the patches
were cut from (the import base plus one commit per patch): the four crates
are identical to it but for their manifests. A following commit runs
`cargo fmt` over the two `pathfinder-json` files the patches left unformatted
(`lib.rs`, `num.rs`); it changes layout only.

| Patch | Applied here |
|---|---|
| 0001 version marker | none (CLI only) |
| 0002 number output | `pathfinder-json`: `num.rs`, `read.rs` |
| 0003 arithmetic precision | `pathfinder-json`: `num.rs` |
| 0004 input literals | `pathfinder-json`: `read.rs` |
| 0005 string repeat | `pathfinder-json`: `lib.rs` |
| 0006 object keys | `pathfinder-json`: `lib.rs` |
| 0007 debug format | none (CLI only) |
| 0008 uncaught errors | `pathfinder-json`: `read.rs` |
| 0009 halt_error | `pathfinder-std`: `defs.jq` |
| 0010 path errors | `pathfinder-core`: `exn.rs`, `filter.rs`, `funs.rs`, `path.rs`, `rc_list.rs`; `pathfinder-std`: `defs.jq`, `lib.rs` |
| 0011 path alternative | `pathfinder-core`: `filter.rs` |
| 0012 remainder | `pathfinder-json`: `lib.rs`, `num.rs` |
| 0013 parse errors | `pathfinder-json`: `funs.rs`, `lib.rs`, `parse_error.rs`, `read.rs` |
| 0014 lazy input | `pathfinder-fmts`: `read/formats.rs`, `read/mod.rs`; `pathfinder-json`: `read.rs` |
| 0015 file locations | none (CLI only) |

### CLI hunks, applied in T-010

These hunks patch jaq's CLI crate (`jaq/`). They are in `crates/pathfinder-jaq`,
which T-010 built from the imported crates. Hunk headers are those of the 3.1.1
cuts.

`crates/pathfinder-jaq/src/jaq/` is `jaq/src/` and
`crates/pathfinder-jaq/src/jaq-all/` is `jaq-all/src/`, both taken with
`git show` from the tree the patches were cut from (the import base plus one
commit per patch), so the CLI hunks are in exactly as the packaged jaq has
them; `jaq-all` has no hunks. One package holds both: its `[lib]` keeps the
name `jaq_all` and its features (`formats`, `std-all`, `mimalloc`) are
`jaq-all`'s and `jaq`'s, so no source file needed an edit to build on the
`pathfinder-*` crates. The binary is named `pathfinder-jaq`, so it collides
with no installed `jaq`, and it is the one difference from the packaged
engine's output that is expected: `--version` prints
`pathfinder-jaq 3.1.1+pathfinder.3` (it takes the name from
`CARGO_PKG_NAME`), which still carries the `+pathfinder` the conformance
test looks for. The dependencies `jaq-all` and `jaq` bring in were locked at
the versions of jaq's `Cargo.lock` at the import base, every one of them. A following commit runs `cargo fmt` over `jq.rs`, which patch 0008
added unformatted; it changes layout only.

| Patch | File | Hunks |
|---|---|---|
| 0001 | `jaq/src/main.rs` | `-57,7` |
| 0007 | `jaq/src/main.rs` | `-32,7` |
| 0008 | `jaq/src/filter.rs` | `-60,6` |
| 0008 | `jaq/src/jq.rs` | new file (329 lines) |
| 0008 | `jaq/src/main.rs` | `-1,6`, `-141,18`, `-163,18`, `-182,24`, `-253,7` |
| 0013 | `jaq/src/jq.rs` | `-13,7`, `-77,6`, `-129,11`, `-141,34`, `-180,29` |
| 0013 | `jaq/src/main.rs` | `-151,7`, `-227,19`, `-255,6`, `-273,6`, `-312,8`, `-323,7` |
| 0015 | `jaq/src/filter.rs` | `-64,6`, `-72,6`, `-88,8` |
| 0015 | `jaq/src/jq.rs` | `-67,6`, `-95,6`, `-128,13`, `-142,6`, `-184,6`, `-195,6`, `-226,6`, `-239,16`, `-425,3` |
| 0015 | `jaq/src/main.rs` | `-60,9`, `-119,6`, `-150,7`, `-160,12`, `-216,10` |

## Upstream unit tests after the patches

`cargo test --workspace --exclude pathfinder-jq`: before the patches all 361
pass; after them 252 pass and 111 fail (363 tests: 0013 adds two to
`pathfinder-json`'s unit tests). Fixing them is T-009.

| Crate | Fail / total | Why |
|---|---|---|
| `pathfinder-core` | 3 / 181 | `defs::tostring_zero` (jaq's `0.0`; 0002), `path::index_access` (`{"a":1} \| .[0]` gives `null` in jaq; 0006), `tests::mul` (`0 * "Hello"`; 0005) |
| `pathfinder-std` | 102 / 102 | test harness, not behaviour: 0009's `halt_error` calls `tojson`, which `jaq-json` defines, and `tests/common/mod.rs` loads only core and std, so no filter compiles |
| `pathfinder-json` | 5 / 50 integration, 0 / 4 unit | `tojson_inf`, `tojson_nan`, `tojson_ninf`, `fromjson_inf` (jaq's NaN/infinity spellings, changed by 0002/0004) and `math_rem` (`%`, changed by 0012) |
| `pathfinder-fmts` | 1 / 26 | `formats::cbor` parses JSON with a non-string object key, which 0013 makes a parse error |
