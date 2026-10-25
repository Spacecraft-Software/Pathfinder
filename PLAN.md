<!--
SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
SPDX-License-Identifier: CC-BY-SA-4.0
-->

# Pathfinder — Plan

MVP: M18–M20 — Pathfinder engine v0.2: jaq's engine in-tree, in-process, at parity

The hard fork of jaq's engine, as specified in [`PRD.md`](PRD.md). Steps 1–3
and M10–M17 are complete; their plan is kept in
[`doc/plans/step-3.md`](doc/plans/step-3.md). Milestone and task numbers
continue from it. The task-level breakdown lives in [`TODO.md`](TODO.md).

Every milestone holds two ratchets: jq 1.8.1's suite never loses a case
(`FLOOR`, `FLOOR-patched`, and from M19 `FLOOR-native`), and the benchmark
budget of PRD R-011 holds. Every expected value comes from running real jq
1.8.1.

## M18 — Import the engine

jaq 3.1.1's `jaq-core`, `jaq-std` and `jaq-json` come into the repository as
Pathfinder crates, with the patch set applied and nothing else changed, so the
first diff against upstream stays reviewable. The JSON and raw readers come
from `jaq-fmts`; the other formats, `jaq-all`, jaq's CLI and `jaq-play` stay
out.

- [x] P-048 Cargo workspace: `crates/pathfinder` (today's `src/`), `crates/pathfinder-core`, `-std`, `-json`
- [x] P-049 Import jaq 3.1.1 at a recorded commit, with `packaging/jaq/0001`–`0015` applied
- [x] P-050 Drop jaq's non-jq formats, `--in-place`, REPL and Windows console code, and their dependencies
- [x] P-051 Replace the `mmap` file load with a safe read; `unsafe_code = "forbid"` in every crate
- [x] P-052 Imported files keep MIT and upstream copyright in `REUSE.toml`; `CREDITS.md` names the import
- [x] P-053 `cargo audit` and a justification for every dependency the import brings in
- [x] P-054 jaq's own unit tests run in the workspace; those that assert jaq's (non-jq) behaviour are updated with measured jq values

## M19 — In-process evaluation

The binary links the engine and evaluates without spawning anything. The
rewriter and prelude still run; only the hand-off changes.

- [x] P-055 jq's options map onto the engine's run options directly, not onto jaq's argv
- [x] P-056 Inputs, multiple files as one stream, `-a`, `--seq` and output written in-process
- [x] P-057 Exit status, uncaught-error reports and `halt` handled in-process
- [x] P-058 `PATHFINDER_ENGINE=native|jaq` selects the engine; CI runs both suites on both
- [x] P-059 `FLOOR-native` ratchet at least 815/820
- [ ] P-060 Benchmarks (`bench/`): startup, parse, filter and output sets; R-011 holds
- [x] P-061 Fuzz targets for the jq-argv parser, the filter parser and the evaluator; no panic

## M20 — jq's semantics in the engine

Repairs move from filter text into the engine, each guarded by the
differential cases that already pin it; the polyfill or rewrite is deleted in
the same change.

- [x] P-062 Errors carry jq's wording natively; `_pf_err` and the report-time translation are retired
- [x] P-063 Assignment creates missing containers as jq does; the vivify rewrite is retired
- [x] P-064 `del` and `delpaths` keep jq's order and semantics; `_pf_delpaths` is retired
- [x] P-065 Indices, slices and operand order are jq's natively; their rewrites are retired
- [x] P-066 Builtins jq 1.8.1 does not define are removed from the engine (PRD R-004)
- [x] P-067 The prelude shrinks to what jq defines in `builtin.jq`; every remaining entry is justified

## M21 — One parser

- [x] P-068 The engine compiles from Pathfinder's jq syntax tree (`src/syntax/`), not jaq's loader
- [x] P-069 jq's compile errors, with jq's wording and positions
- [ ] P-070 Modules: `import`, `include`, search paths and `modulemeta` as jq 1.8.1 has them (optional)

## M22 — Regular expressions

- [ ] P-071 A pure-Rust engine with lookaround and Unicode `\b` (`fancy-regex` or equivalent), measured against jq's regex suite
- [ ] P-072 Oniguruma's flags and capture semantics for the cases in jq's suite

## M23 — Numbers and the remaining gaps

- [ ] P-073 Decimal literals keep their text until computed on (`1E+1000`, big integers) as jq's decNumber does
- [ ] P-074 `try input` and `break`, and the other evaluator quirks in jq's suite
- [ ] P-075 `--stream` and `--stream-errors` (optional)
- [ ] P-076 `input_line_number` and `$__loc__` (optional)
- [ ] P-077 jq 1.8.1's suite at 818/820 or better (PRD R-003)

## M24 — Release

- [ ] P-078 The jaq engine path and `packaging/jaq/` are removed; Nix builds one binary
- [ ] P-079 24 h of fuzzing clean; benchmarks recorded in `doc/DIVERGENCES.md`
- [ ] P-080 README, AGENTS.md, SKILL.md, the Texinfo manual and `DIVERGENCES.md` describe the engine
- [ ] P-081 Bravais switched to the native engine; version 0.2.0
