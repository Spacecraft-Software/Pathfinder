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
| `crates/pathfinder-all` | `jaq-all/src/` (in `pathfinder-jaq` until M19, below) | 0.3.0 |
| `crates/pathfinder-jaq` | `jaq/src/` (T-010, below; a library since M19) | 3.1.1 |

`jaq-play`, `jaq-all`'s examples and the `jaq` crate's `tests/` (golden tests
of the CLI, mostly of the formats and `--in-place` that M18 removes) were not
imported.

`jaq-fmts` is its own crate rather than part of `pathfinder-json`: the
functions jaq's CLI reads and writes with (`read::read`, `read::parse`,
`read::read_string`, `read::bytes_str`, `write::write`) live in
`read/formats.rs` and `write/formats.rs`, behind the `all` feature, and
dispatch over every format at once. There is no JSON-and-raw subset to copy
without editing those files, so it is copied whole and stays diffable against
upstream; the YAML, TOML, CBOR, XML and CSV/TSV code and their dependencies
went in T-004 (below).

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

## Removed after the import

What jq does not have left the crates after the import, each removal a commit
of its own, so the import and patch commits stay diffable against upstream.

T-004 (PLAN P-050):

- `pathfinder-fmts`: the CBOR, YAML, TOML and XML readers and writers, the
  CSV/TSV readers, the filters over them (`fromcbor`, `tocbor`, `fromyaml`,
  `toyaml`, `fromtoml`, `totoml`, `fromxml`, `toxml`, `fromcsv`, `tocsv`,
  `fromtsv`, `totsv`; none is a jq 1.8.1 builtin, measured), and the
  features they hung off (`all`, `cbor`, `toml`, `xml`, `yaml`, `tabular`).
  `Format` keeps `Raw`, `Raw0` and `Json`; `read_string` and `bytes_str`,
  which existed only for the text formats, are gone, and `read`/`parse`
  lost their `&str` argument. `write/tabular.rs` stays: it is jq's `@csv`
  and `@tsv`. `Format::determine` is gone too, so a file is JSON whatever
  its extension, as in jq (jaq read `x.yaml` as YAML). The crate's tests
  were all of removed formats and went with them.
- `pathfinder-jaq`: `-i`/`--in-place`, the REPL (`repl`, `funs.rs`),
  `windows.rs` and its two `cfg(windows)` arms, `jaq-all`'s `formats`
  feature, and the help text for them.
- Dependencies gone from `Cargo.lock`: `ciborium-io`, `ciborium-ll`, `half`,
  `crunchy`, `saphyr-parser`, `arraydeque`, `smallvec`, `toml-span`,
  `xmlparser`, `cfg-if`, and the CLI's `rustyline`, `dirs`, `tempfile` and
  their trees. `aho-corasick` and `base64` stay: `pathfinder-std`'s `format`
  feature (`@base64`, `@html`, …) needs them, and `@tsv` uses
  `aho-corasick`.

T-005 (PLAN P-051): `read::load_file` reads the file with `std::fs::read`
instead of memory-mapping it with `memmap2`, the one `unsafe` outside
Windows code; `memmap2` leaves `Cargo.lock`. Every file the CLI loads goes
through it: input files, `--rawfile`, `--slurpfile` and imported JSON
modules. Error text is unchanged (a directory was already read by the
fallback, since mapping it fails). Measured with `pathfinder-jaq length`,
release build, `perf stat -e instructions:u,cycles:u`, one run per
invocation, 7 runs each, medians:

| Input | instructions:u before | after | cycles:u before | after |
|---|---|---|---|---|
| `big.json` (29.4 MB, one value) | 2 001 889 082 | 2 001 902 821 (+0.001%) | 1 073 151 417 | 966 605 360 |
| `big.jsonl` (14.4 MB, many values) | 2 323 415 466 | 2 323 691 595 (+0.012%) | 1 293 586 616 | 1 330 212 696 |

The cycle counts are noise on this host (the spread across the seven runs
is 27–39% of the median, either way). User page faults fall from 696 to
253 and from 353 to 133: the read faults its buffer in once, where the
map faulted file pages lazily. The copy itself is kernel time, which
`:u` events do not see; `hyperfine` (15 runs) put system time at
21 → 25 ms and 71 → 87 ms, with wall-clock means 512 → 544 ms and
649 → 691 ms against a standard deviation of 77–122 ms. No difference
stands out of the noise.

T-006 (PLAN P-051): every engine crate and `pathfinder-jaq` takes the
workspace lint set (`[lints] workspace = true`), so `unsafe_code = "forbid"`
holds in every crate; with `memmap2` gone and `windows.rs` removed, none
needed a change for it. What the pedantic set finds in upstream code is
answered at each crate root, not fixed in place, so the code stays close to
upstream: `#![expect(…, reason = …)]` for a lint that fires in every build
configuration, grouped by reason (style, public API, numeric casts), and
`#![allow(…, reason = …)]` for one that fires only in some (a test build, a
feature set: `enum_glob_use` is silent in test builds, the `serde` module
exists only with that feature, `pathfinder-std` without its default features
has dead code), where `expect` would go unfulfilled. The integration tests
use `allow`, because T-009 rewrites them. "Every configuration" was measured:
`cargo clippy` over the workspace, with `--all-targets`, with
`--lib --bins --profile test`, and per crate (`-p`) in the same three ways;
all are clean with `-D warnings`, except that `cargo clippy -p
pathfinder-json --all-targets` alone does not compile `pathfinder-json`'s
tests (they need its `serde` feature, which upstream also gets only from the
workspace). Fixed rather than answered: `rust-version` is 1.82 for
`pathfinder-json` and the crates that depend on it, since the patch set's
number output uses `iter::repeat_n` (`incompatible_msrv`; 1.82 is what clippy
reports, no 1.82 toolchain was tried) and 1.81 for `pathfinder-core` and
`pathfinder-std`, the release that stabilised `#[expect]` and lint `reason`s
these attributes use; the redundant imports in the
`pathfinder-fmts` files T-004 rewrote; and the CLI's `#[allow(dead_code)]`,
now `#[expect]` with a reason.

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
| 0016 trailing comment | `pathfinder-core`: `load/lex.rs` (added after M18: an upstream bug found by fuzzing, T-019) |

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

## Dependencies

What the import brings in: the direct dependencies of the four imported crates
(PLAN P-053), at the versions `Cargo.lock` holds. Sibling crates and
dev-dependencies (`serde_json`, already the shim's) are left out. A dependency
that only a non-jq format needed was removed in T-004 and is listed as such.

| Crate | Version | Used by | Purpose | Disposition |
|---|---|---|---|---|
| `dyn-clone` | 1.0.20 | core | Cloning boxed update closures (`filter.rs`) | Kept |
| `once_cell` | 1.21.4 | core | Lazily evaluated, shared output lists (`rc_lazy_list.rs`) | Kept |
| `typed-arena` | 2.0.2 | core | Arena that owns the loaded module sources (`load/mod.rs`) | Kept |
| `arbitrary` | 1.4.2 | core, optional | Generates parser input for fuzzing (`load/arbitrary.rs`); off unless the `arbitrary` feature is on, which only jaq's `fuzz/` crate enables | Kept for T-019 |
| `bstr` | 1.13.1 | std, json, fmts | Byte strings: jq strings are not required to be valid UTF-8 internally | Kept |
| `jiff` | 0.2.35 | std, `time` | Date and time builtins (`strftime`, `mktime`, `todate`, …) | Kept |
| `regex-bites` | 0.1.6 | std, `regex` | `test`, `match`, `capture`, `sub`, … | Replaced in M22 (T-042) by `crates/pathfinder-regex`, Pathfinder's port of the Oniguruma 6.9.10 jq 1.8.1 matches with; it has no dependencies |
| `log` | 0.4.29 | std, `log` | Routes `debug` and `stderr` messages to the CLI's logger | Kept; revisit when M19 writes them in-process |
| `libm` | 0.2.16 | std, `math` | The C math library functions jq exposes (`ilogb`, `scalbln`, `frexp`, …) | Kept |
| `aho-corasick` | 1.1.5 | std (`format`), fmts (`tabular`) | Escaping for `@sh`, `@html` and the other `@format`s; `@csv`/`@tsv` writing in fmts | Kept |
| `base64` | 0.22.1 | std (`format`), fmts (`yaml`) | `@base64`, `@base64d`; YAML binary values | Kept for std; the fmts use was removed in T-004 |
| `urlencoding` | 2.1.3 | std (`format`) | `@uri` | Kept |
| `bytes` | 1.12.1 | json, fmts | Reference-counted byte buffers behind string values | Kept |
| `foldhash` | 0.1.5 | json | Hasher for objects' `IndexMap` | Kept |
| `hifijson` | 0.5.0 | json | The JSON lexer and parser for input and `fromjson` | Kept |
| `indexmap` | 2.10.0 | json | Objects that keep insertion order | Kept |
| `num-bigint` | 0.4.8 | json, fmts (`cbor`) | Integers beyond `isize` in literals | Kept for json; the fmts use was removed in T-004 |
| `num-traits` | 0.2.19 | json | Conversions between the number representations | Kept |
| `ryu` | 1.0.20 | json | Was jaq's float formatter; unused since patch 0002, which prints numbers jq's way | Removed |
| `self_cell` | 1.3.0 | json | Self-referential value that keeps parsed input and its buffer together (`funs.rs`) | Kept |
| `serde_core` | 1.0.229 | json, optional | `serde` support for `Val`; on only through the `serde` feature, which the crates' tests enable | Kept (tests) |
| `memmap2` | 0.9.11 | fmts | Mapped input files into memory; the only `unsafe` the import needed | Replaced by `std::fs::read` in T-005 |
| `ciborium-io`, `ciborium-ll` | 0.2.2 | fmts (`cbor`) | CBOR | Removed in T-004 |
| `saphyr-parser` | 0.0.8 | fmts (`yaml`) | YAML | Removed in T-004 |
| `toml-span` | 0.7.1 | fmts (`toml`) | TOML | Removed in T-004 |
| `xmlparser` | 0.13.6 | fmts (`xml`) | XML | Removed in T-004 |

### `cargo audit`

Run 2026-10-11 from the workspace root with `cargo-audit` 0.22.2 (from
`nixpkgs#cargo-audit`), advisory database fetched on the day: 1296 advisories,
`RustSec/advisory-db` commit `7eebec69c352c7191b1f13eb95dd510eeca5d1de`
(2026-10-09). 79 crates scanned in `Cargo.lock`: **no vulnerabilities, no
warnings** (unmaintained, unsound or yanked).

```sh
nix shell nixpkgs#cargo-audit -c cargo audit
```

## Upstream unit tests after the patches

`cargo test --workspace` (unit, integration and doc tests). Before the
patches all 361 of the imported tests pass. After them 111 of 363 failed
(0013 adds two to `pathfinder-json`'s unit tests). T-009 brought that to 2,
and both are now gone: `pathfinder-fmts`' format tests left with the formats
in T-004, and the `-0` bug below is repaired. None fails.

Every changed expectation names its patch in a comment and was measured with
jq 1.8.1. Where the original program is an error in jq, the program changed
too, to the nearest one jq answers:

| Crate | Test | Was | Now (jq 1.8.1) | Patch |
|---|---|---|---|---|
| `pathfinder-core` | `defs::tostring_zero` | `"0.0"` | `"0"` | 0002 |
| `pathfinder-core` | `path::index_access` | `{"a":1} \| .[0]` gives `null` | jq errors (`Cannot index object with number`); the test now asserts `.[0]?` gives nothing, since the engine's error text is still jaq's | 0006 |
| `pathfinder-core` | `tests::mul` | `"Hello" \| 0 * .` gives `null` | `""` | 0005 |
| `pathfinder-std` | all 102 | harness | `tests/common/mod.rs` loads `pathfinder-json`'s definitions and natives: 0009's `halt_error` calls `tojson`, which that crate defines | 0009 |
| `pathfinder-std` | `defs::recurse_fib_100` | `"354224848179261915075"` | `"354224848179262000000"` | 0002, 0003 |
| `pathfinder-json` | `funs::fromjson_inf` | `"Infinity +Infinity -Infinity" \| [fromjson \| tostring]` | jq rejects several values in one `fromjson`; `[("Infinity", "+Infinity", "-Infinity") \| fromjson \| tostring]` gives `["1.7976931348623157e+308", "1.7976931348623157e+308", "-1.7976931348623157e+308"]` | 0002, 0004, 0013 |
| `pathfinder-json` | `funs::tojson_nan`, `tojson_inf`, `tojson_ninf` | `0.0 / 0.0 \| tojson` gives `"NaN"`, … | jq rejects division by zero; `nan`, `infinite`, `-infinite \| tojson` give `"null"`, `"1.7976931348623157e+308"`, `"-1.7976931348623157e+308"` | 0002 |
| `pathfinder-json` | `funs::math_rem` | jaq's float remainder | jq's: both operands truncated to integers, e.g. `-2 % 2.1` is `0`, `2.1 % 3` is `2`; `2.1 % 0` is an error, not NaN | 0012 |

`math_rem` still asserts jaq's wording for `% 0` (`cannot calculate -2 % 0`;
jq says `number (-2) and number (0) cannot be divided (remainder) because the
divisor is zero`). The patches did not change it; the engine takes jq's
wording in M20 (P-062).

### The `-0` bug from patch 0003, repaired

T-009 found it: `defs::significand_range` failed. 0003 makes `0 * -1` the
float `-0` (`impl Mul for Num`), so that it prints as jq's `-0`, and
`Num::as_isize`/`as_pos_usize` accepted only `Int` and `BigInt`, so every
builtin or index that wants an integer rejected it with `cannot use -0 as
integer`. jq has no integers and takes it as 0. `significand` is
`scalbln(.; ilogb | -1 * .)`, so it failed on every input whose exponent is 0,
through the shim too.

0003 now takes a zero float as the integer 0 in `as_isize` and
`as_pos_usize`, and indexes an array with it (`index_opt`). The repair is in
`packaging/jaq/0003-arithmetic-precision.patch` as well as in
`pathfinder-json`, so the packaged engine has it too. Measured against jq
1.8.1, and pinned by a `PATCHED_CASES` case in `tests/differential.rs`:

| Program | jq 1.8.1 | before | after |
|---|---|---|---|
| `1 \| significand` | `1` | error | `1` |
| `ldexp(1; 0 * -1)` | `1` | error | `1` |
| `[1, 2] \| .[0 * -1]` | `1` | error (`Cannot index array with number`) | `1` |
| `"abc" \| .[0 * -1:]` | `"abc"` | error | `"abc"` |
| `[1, 2] \| del(.[0 * -1])` | `[2]` | error | `[2]` |
| `[0 * -1] \| implode` | `"\u0000"` | error | `"\u0000"` |

### Known coupling: `pathfinder-std` needs `pathfinder-json`

Patch 0009 rewrites `halt_error` in `pathfinder-std/src/defs.jq` to call
`tojson`, a native that `pathfinder-json` defines. Upstream, `jaq-std`'s
definitions compile against `jaq-core`'s and its own natives alone; now
compiling them without `jaq_json::funs()` fails, whatever the filter. With
a loader over `jaq_core::defs().chain(jaq_std::defs())` and a compiler over
`jaq_core::funs().chain(jaq_std::funs())`, the program `.` fails with
`[(File { code: "", path: () }, [("tojson", Filter(0))])]`, an undefined
`tojson/0`.

Pathfinder always loads all three, so no `jq` call is affected, and the
crate's tests now load the JSON natives too. A standalone user of the std
crate would be. Two ways to repair it in the sources: move `halt_error` into
`pathfinder-json/src/defs.jq`, next to `tojson`, or merge the crates when the
engine's crate layout is settled.

## jaq 3.1.2 (the packaged engine)

Bravais's nixpkgs moved to jaq 3.1.2 (2026-10-23), and patch `0008` stopped
applying. The patch commits were replayed onto 3.1.2 in jaq's source tree:
3.1.2 passes `input_filename` to `jaq_all::data::run` and keeps it in `Data`
(the CLI hunks now pass `Val::Null` there, the patched CLI's
`$!input_filename` binding still answering), rounds with helpers of its
own (`0010` merged), and fixed the lexer bug of `0016` itself. `0008`,
`0010` and `0015` have 3.1.2 cuts, applied without fuzz; on the patched
3.1.2 the suites pass at 815/820, the same five failures as on 3.1.1. The
in-tree crates stay on 3.1.1.

## M19: the CLI as a library

M19 (PLAN P-055–P-057) runs the engine in the `pathfinder` binary's own
process, through jaq's CLI. To make that CLI a library, `crates/pathfinder-jaq`
was split:

| Now | Was | Holds |
|---|---|---|
| `crates/pathfinder-all/src/` | `crates/pathfinder-jaq/src/jaq-all/` | `jaq-all`, lib `jaq_all`, unchanged |
| `crates/pathfinder-jaq/src/{cli,filter,jq,tests}.rs`, `help.txt` | `crates/pathfinder-jaq/src/jaq/` | jaq's CLI modules |
| `crates/pathfinder-jaq/src/lib.rs` | `crates/pathfinder-jaq/src/jaq/main.rs` | the driver, lib `jaq` |
| `crates/pathfinder-jaq/src/main.rs` | — | the `pathfinder-jaq` binary: mimalloc and `jaq::main()` |

The CLI sources keep their `crate::` and `jaq_all::` paths, so they still diff
cleanly against `jaq/src/` in the patched tree. The changes, all marked
`Pathfinder:` in the source:

- `main.rs` → `lib.rs`. `main` is `jaq::main`, minus the global allocator,
  which only a binary may set. The logger set-up is `init_logging`. The new
  `run(cli, input, segments, out) -> u8` is `real_main` plus jaq's error
  report: stdin is any `Read`, the concatenated files' `Segment`s are passed
  in rather than read from `PATHFINDER_INPUTS`, and stdout is any `Write`,
  buffered by the caller (`main_io` keeps jaq's `with_stdout` choice). `halt`
  and `halt_error` return their status (its low byte, as the operating system
  keeps it) instead of calling `process::exit`, so the caller can flush first.
- `cli.rs`: `Cli::parse_from` parses any argument list; `Cli::parse` reads
  `std::env::args_os()` through it.
- `jq.rs`: `Segment::new`; `Position::filename`.
- `input_filename` is a native function (`pathfinder-all`'s `data::funs`)
  reading `Data::filename`, which the driver points at its reader's
  `Position`: `null` before any input is read, then the file the reader has
  open, as jq 1.8.1 answers. It replaces the CLI's `$!input_filename`
  variable, which was `"<stdin>"` from the start (M20, T-028).

`jq.rs`, `filter.rs` and `Error`'s rendering are untouched, so the CLI hunks
of patches 0008, 0013 and 0015 above still describe the code. A fix to them
lands twice, as AGENTS.md says. The driver-only changes above have no
counterpart in `packaging/jaq/`: they change no behaviour of a jaq binary,
which `pathfinder-jaq` run as `PATHFINDER_JAQ` keeps checking.

### mimalloc

jaq's binary sets mimalloc as its global allocator; the `pathfinder` binary
now does too (its `mimalloc` feature, on by default). Measured with
`perf stat -e cycles:u` on 200 000 objects (22 MB NDJSON),
`-s 'group_by(.tags[2]) | map(length)'` took 5.1e9 user cycles on glibc's
allocator and 2.5e9 with mimalloc, level with `pathfinder-jaq` (2.4e9). The
system allocator would break PRD R-011's 10% budget against jaq by a factor of
two. mimalloc is the dependency jaq already had, at the version locked at the
import base.

## M20: jq's semantics in the engine

Repairs move from the prelude and the rewriter into the crates, for the
native engine; the jaq engine keeps the text repairs until P-078, so these
changes land once, in the crates, not in `packaging/jaq/`.

- `pathfinder-std`'s `ValT` gains `jq_type` and `jq_quoted` (jq's
  `number (1)`, `array ([1,2,3,4,5,...)`), which builtins use to raise
  jq 1.8.1's messages where they fail: the regex builtins (input, regex
  and flags of the wrong type, an unknown flag string), `explode`,
  `ascii_downcase`/`ascii_upcase`, `startswith`/`endswith`/`ltrimstr`/
  `rtrimstr`, `implode`, `sort`, `sort_by`/`group_by`/`min_by`/`max_by`,
  `strptime`. `reverse` and `implode` follow jq's definitions (`reverse` of
  a non-array; U+FFFD for an invalid code point, where jaq read a negative
  number as a byte). In `defs.jq`, `unique` fails as `sort` does and
  `getpath` names a path that is no array. jaq's unit test of `implode`
  now expects jq's answer.
- The core's errors are jq's (T-022): `ValT` gains `jq_type`, `jq_string`
  and `jq_quoted` (moved up from `pathfinder-std`), and `Error::typ`,
  `Error::math` and `Error::index` build jq 1.8.1's messages (`Cannot iterate
  over number (1)`, `number (1) and string ("a") cannot be added`, `Cannot
  index number with string "a"`, `... number required`) where jaq said
  `cannot use 1 as iterable (array or object)`, `cannot calculate 1 + "a"`
  and `cannot index 1 with "a"`. Also jq's: `keys`/`to_entries` on a value
  without keys, `contains` between types, `@csv`/`@tsv`, a native `@sh`,
  native `min`/`max`, the `…_by` functions on an object, `fromdateiso8601`,
  an object slice without both bounds. jaq's tests asserting jaq's wording
  now expect jq's.
- `input_line_number` is a native function on `Data::line_number`, the
  reader position's line count (`Position::line_number`).
- Arithmetic and comparisons run the right operand in the outer loop, its
  error first, as jq does (`cartesian_rl`); object construction keeps the
  first entry outer through a term of its own, `Term::ObjCat`, where jaq
  summed the entries with `+`. Math functions taking a C `int` (`ldexp`,
  `scalbln`, `jn`, `yn`) truncate their argument as C's cast does, and
  `scalb` is C's (NaN for an exponent that is no integer). jaq's tests of
  those orders and of `scalb` now expect jq's answers.
- Numbers as indices and bounds, as jq 1.8.1 takes them (T-026): a
  fractional index is truncated and NaN, infinite or too large indexes
  nothing (`Num::as_jq_index`), in reads, paths (`path(.[1.5])` is
  `[1.5]`) and updates; slice bounds round outwards after a negative one is
  counted from the end, NaN is no bound and a non-number is jq's error
  (`Num::as_jq_bound`); `null` slices to `null` and updates as `[]`. `/` by
  zero is jq's error, so `nan` and `infinite` are native rather than `0/0`
  and `1/0`; unary minus words jq's error; `range` with no step yields
  nothing. jaq's tests of those now expect jq's answers.
- Assignment as jq 1.8.1 does it (T-024): `map_index` creates missing
  containers (`null` updated as the object or array its key implies), pads
  an array past its end, deletes in place keeping the other keys' order
  (`shift_remove`, where jaq swapped the last key in), and raises jq's
  errors (`Out of bounds negative array index`, `Array index too large`,
  `Cannot set array element at NaN index`, `Cannot update field at array
  index of array`, `Cannot update string slices`, `A slice of an array can
  only be assigned another array`). An update whose target is no path
  expression fails as `path(target)` does (`update_jq`), which only a
  target that is not a plain path pays for: holding the input a second time
  would make every in-place update copy it.
- Deletion as jq 1.8.1 does it (T-025): a native `_delpaths` in
  `pathfinder-json` is jq's `jv_delpaths` (paths sorted, grouped by key and
  deleted together, jq's errors), and `pathfinder-json`'s `defs.jq`
  redefines `delpaths` with it and `del(f)` as jq's
  `delpaths([path(f)])`. `pathfinder-core` keeps jaq's definitions, so its
  tests still run without the JSON crate.
- jq 1.8.1's own definitions (`src/builtin.jq`) of `tostream`,
  `fromstream`, `truncate_stream`, `IN`, `INDEX`, `JOIN`, `trimstr`,
  `have_decnum`, `have_literal_numbers`, `skip`, `limit`, `nth`, `flatten`,
  `join` and `pick` are the engine's, in `pathfinder-json`'s `defs.jq`
  (T-027), where jaq had none or its own. jaq's tests of `flatten` on an
  object and a number now expect jq's answers.
- Native with jq 1.8.1's semantics and messages: `has` (a string key for an
  object, a truncated number for an array, `null` has nothing), `tonumber`
  (a string whole as C's `strtod` reads it, a literal's text kept),
  `toboolean`, `utf8bytelength`, `bsearch`, `trim`/`ltrim`/`rtrim`; jq's
  definitions of `from_entries` and `with_entries`. Object construction
  refuses a key that is no string (`Cannot use null (null) as object key`),
  where jaq built objects that are no JSON.
- jaq's own helpers are internal (P-066): `isarray`, `isstring`, `tobytes`,
  `key_values`, `matches`, `halt/1`, `ilogb` and the rest (28 names jq 1.8.1
  does not define) are renamed with a leading `_` in the engine's natives,
  definitions and tests, so a filter cannot reach them even where the
  front end does not check; `@htmld`, which jq lacks, is gone.
- `@base64d` and `@urid` are jq's (P-067): `@base64d` decodes up to the
  first `=`, ignores missing padding and unused low bits, and raises jq's
  two messages; `@urid` rejects a malformed `%` escape and bytes that are
  not UTF-8. Bytes `@base64d` yields that are not UTF-8 become U+FFFD as
  jq's `jvp_utf8_next` reads them (`jq_lossy_utf8`). `format/1`, a C
  builtin in jq, is defined in `pathfinder-all`'s new `defs.jq`, the one
  place that sees `jaq_fmts`'s `@csv`/`@tsv`.
- jq's evaluation order (PR #19's review): an error of a binary operator's
  right operand is raised before the left runs (`cartesian_rl`); natives
  bind their arguments last outermost, as jq calls its C builtins
  (`bind_native`; `range/3` is `_range` behind a definition, `scan`,
  `split`, `splits`, `sub` and `gsub` bind `$` parameters as jq's do); an
  update to nothing through an absent child leaves it absent
  (`ValT::jq_absent`); a slice of `null` is updated from `null`;
  `setpath` is the native `_setpath`, which owns its input.
- The main module's parser is a parameter (M21, T-031): `load::MainParser`,
  `Loader::with_main_parser`, `jaq::Cli::front_end`; `parse::Module` and
  its `Dep` are public so a front end can build one, and `jaq` re-exports
  `jaq_core`. With none given, jaq's parser reads the program as before.

## M22: Oniguruma, as jq 1.8.1 runs it

`crates/pathfinder-regex` is not imported from jaq. It is Pathfinder's
Rust rendering of the one configuration of Oniguruma that jq 1.8.1's
release binary runs: the Oniguruma **6.9.10** vendored in jq's tarball
(`vendor/oniguruma`; the binary is built `--with-oniguruma=builtin`),
compiling with `ONIG_SYNTAX_PERL_NG`, UTF-8 and
`ONIG_OPTION_CAPTURE_GROUP` (`f_match` in jq's `src/builtin.c`). The
tarball is jq's release `jq-1.8.1.tar.gz`, sha256
`2be64e7129cecb11d5906290eba10af694fb9e3e7f9fc208a311dc33ca837eb0`.

- `src/tables.rs` is Oniguruma's Unicode data (properties, case folding,
  grapheme cluster breaks, the ISO-8859-1 character types), written by
  `tools/onig-tables.py` from `unicode_property_data.c`,
  `unicode_fold_data.c`, `unicode_egcb_data.c` and `unicode.c`; it is
  regenerated, never edited.
- `src/parse.rs`, `src/tune.rs`, `src/compile.rs` and `src/exec.rs`
  transcribe `regparse.c`, the parts of `regcomp.c` that change what is
  accepted or matched, and `regexec.c`'s matcher, keeping Oniguruma's
  function names and control flow, so that every pattern is accepted,
  rejected, worded and matched as jq does. `src/error.rs` holds
  `regerror.c`'s messages.
- Not carried over: Oniguruma's optimisations, which do not change a
  result (search skips, exact-string tables, the anychar-star and
  peek-next instructions), and what TODO T-044 lists (subexpression
  calls, the absent operator, callouts), which are rejected.
- `pathfinder-std`'s `_match_impl` is jq's `f_match`, and its `defs.jq`
  takes jq 1.8.1's `match`, `test`, `capture`, `scan`, `splits`, `split/2`,
  `sub` and `gsub` (`src/builtin.jq`). jaq's unit tests of its own regex
  internals (`funs::regex`) now test those filters, and `defs::gsub_many`
  expects jq's answer, `["XxYy","!Xx!Yy"]`, where jaq's was the product
  of the replacements.

The packaged jaq keeps `regex-bites` and the shim's prelude repairs until
P-078 retires it; the native engine needs neither.

### Benchmark

Measured 2026-10-30 on the dev host, best of three runs, the three
binaries side by side in one session: this branch's native engine, the
native engine on `main` before M22 (`regex-bites` with the shim's prelude
repairs), and jq 1.8.1. 100 000 short strings for the per-element
filters, a 100 KB text for the global ones.

| Workload | M22 | before | jq 1.8.1 |
|---|---:|---:|---:|
| `[.[] \| select(test("^[a-z]+[0-9]+$"))]` | 1839 ms | 1936 ms | 1616 ms |
| `[.[] \| capture("(?<w>[a-z]+)(?<n>[0-9]*)")]` | 2858 ms | 3358 ms | 2111 ms |
| `[.[] \| sub("[0-9]+"; "#")]` | 3555 ms | 3734 ms | 4200 ms |
| `[scan("\\w+")]` | 78 ms | 240 ms (wrong: 25 283 words, jq 21 947) | 7722 ms |
| `gsub("a"; "b")` | 203 ms | 1837 ms | 16932 ms |
| `[splits(" +")]` | 200 ms | 33 ms | 17045 ms |

jq is quadratic on the global filters: `f_match` counts every offset
from the start of the string, and its `sub`/`splits` slice by code point
once per match. Pathfinder computes the same values with a counter that
goes on from the last offset and a string slice that remembers one
string's character starts. `splits` stays slower than before: jq's
definition is a `foreach` over the matches with an object per step, where
jaq's was one native call; it is kept for jq's semantics (it is a path
expression: `path(splits(","))` gives jq's paths).

## Licence of Pathfinder's changes

The text taken from jaq keeps jaq's MIT licence and its copyright notice
(Standard §4.2). Pathfinder's changes to it, the patch set in
`packaging/jaq/` and the files Pathfinder added among the imported ones (for
example `pathfinder-json/src/parse_error.rs`, `pathfinder-jaq/src/jq.rs`)
are offered under `GPL-3.0-or-later OR MIT`, at the recipient's choice.
`REUSE.toml` records each such file as `MIT AND (GPL-3.0-or-later OR MIT)`:
as a whole, every file under `crates/pathfinder-{core,std,json,fmts,all,jaq}`
can always be used under MIT, as jaq can. The shim crate, `crates/pathfinder`,
is `GPL-3.0-or-later` only.

`crates/pathfinder-regex` carries its licences inline. Its tables are
Oniguruma's data, under Oniguruma's `BSD-2-Clause` and K.Kosako's notice;
the files that transcribe Oniguruma's code are
`BSD-2-Clause AND (GPL-3.0-or-later OR MIT)`, keeping that notice; the
rest are Pathfinder's, `GPL-3.0-or-later OR MIT`.
