<!--
SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
SPDX-License-Identifier: CC-BY-SA-4.0
-->

# Pathfinder — TODO

MVP: M18–M20 — Pathfinder engine v0.2: jaq's engine in-tree, in-process, at parity

Task-level steps for [`PLAN.md`](PLAN.md); each names the plan item it
serves. Later milestones are broken down when they start, not before.

## M18 — Import the engine

- [x] T-001 (P-048) Turn the repository into a Cargo workspace; move `src/`, `tests/` and the binary manifest to `crates/pathfinder` without behaviour change; CI, Makefile, Nix and REUSE paths follow
- [x] T-002 (P-049) Unpack jaq 3.1.1's source from nixpkgs, record its commit and hash in `crates/UPSTREAM.md`
- [ ] T-003 (P-049) Copy `jaq-core`, `jaq-std`, `jaq-json` and the JSON/raw parts of `jaq-fmts` into `crates/`, renamed; apply `packaging/jaq/0001`–`0015` in order
- [ ] T-004 (P-050) Remove the YAML, TOML, CBOR, XML, CSV/TSV readers and writers, `--in-place`, and their dependencies; the feature flags they hang off go too
- [ ] T-005 (P-051) Replace `memmap2` with `std::fs::read`; measure file-input cost before and after
- [ ] T-006 (P-051) Add `unsafe_code = "forbid"` and Pathfinder's lint set to each new crate; fix or justify every lint
- [ ] T-007 (P-052) `REUSE.toml` entries for the imported paths (MIT, jaq's copyright); `LICENSES/MIT.txt`; `CREDITS.md` section for the engine
- [ ] T-008 (P-053) `cargo audit`; list each imported dependency with its purpose in `crates/UPSTREAM.md`
- [ ] T-009 (P-054) Run jaq's unit tests; replace assertions of jaq-only behaviour with values measured from jq 1.8.1, or delete tests of removed features
- [ ] T-010 (P-049) Build a `jaq`-compatible binary from the imported crates and run the existing suites against it as `PATHFINDER_JAQ`: identical to the packaged jaq

## M19 — In-process evaluation

- [ ] T-011 (P-055) A run-options struct built from `jqargs::JqArgs`; delete the jq-to-jaq argv translation for the native engine only
- [ ] T-012 (P-056) Input readers (stdin, files as one stream, `-R`, `-s`, `-n`, `--seq`, `--raw-input0` refusal kept) in-process, with jq's error locations
- [ ] T-013 (P-056) Output writer with jq's formatting options (`-c`, `--indent`, `--tab`, `-S`, `-C`, `-j`, `-r`, `-a`, `--seq`) in-process; `src/post.rs` retired for the native engine
- [ ] T-014 (P-057) Exit status, uncaught errors, `halt`, `halt_error` and broken-pipe handling in-process
- [ ] T-015 (P-058) `PATHFINDER_ENGINE` switch; `--version`/`--explain` report the engine in use
- [ ] T-016 (P-058) CI matrix: differential and conformance on `native` and on `jaq`
- [ ] T-017 (P-059) `tests/jq-suite/FLOOR-native`; the conformance runner selects it for the native engine
- [ ] T-018 (P-060) `bench/`: startup (`-n .`), 30 MB single-line and line-delimited input, a filter-heavy set, an output-heavy set; record jq 1.8.1, jaq 3.1.1 and native figures
- [ ] T-019 (P-061) `cargo fuzz` targets: `jqargs::parse`, `syntax::parse`, and evaluation of fuzzed filters over fuzzed input with a step limit

## Standing

- [ ] T-020 Review jaq's releases after the import quarterly; port fixes by hand, never send anything upstream (§6.4)
- [ ] T-021 Keep `doc/DIVERGENCES.md` true at every milestone; a closed divergence moves to the repaired table with its measurement
