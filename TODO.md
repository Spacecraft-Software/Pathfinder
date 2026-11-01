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
- [x] T-003 (P-049) Copy `jaq-core`, `jaq-std`, `jaq-json` and the JSON/raw parts of `jaq-fmts` into `crates/`, renamed; apply `packaging/jaq/0001`–`0015` in order
- [x] T-004 (P-050) Remove the YAML, TOML, CBOR, XML, CSV/TSV readers and writers, `--in-place`, and their dependencies; the feature flags they hang off go too
- [x] T-005 (P-051) Replace `memmap2` with `std::fs::read`; measure file-input cost before and after
- [x] T-006 (P-051) Add `unsafe_code = "forbid"` and Pathfinder's lint set to each new crate; fix or justify every lint
- [x] T-007 (P-052) `REUSE.toml` entries for the imported paths (MIT, jaq's copyright); `LICENSES/MIT.txt`; `CREDITS.md` section for the engine
- [x] T-008 (P-053) `cargo audit`; list each imported dependency with its purpose in `crates/UPSTREAM.md`
- [x] T-009 (P-054) Run jaq's unit tests; replace assertions of jaq-only behaviour with values measured from jq 1.8.1, or delete tests of removed features
- [x] T-010 (P-049) Build a `jaq`-compatible binary from the imported crates and run the existing suites against it as `PATHFINDER_JAQ`: identical to the packaged jaq

## M19 — In-process evaluation

- [x] T-011 (P-055) A run-options struct built from `jqargs::JqArgs`; delete the jq-to-jaq argv translation for the native engine only
- [x] T-012 (P-056) Input readers (stdin, files as one stream, `-R`, `-s`, `-n`, `--seq`, `--raw-input0` refusal kept) in-process, with jq's error locations
- [x] T-013 (P-056) Output writer with jq's formatting options (`-c`, `--indent`, `--tab`, `-S`, `-C`, `-j`, `-r`, `-a`, `--seq`) in-process; `src/post.rs` retired for the native engine
- [x] T-014 (P-057) Exit status, uncaught errors, `halt`, `halt_error` and broken-pipe handling in-process
- [x] T-015 (P-058) `PATHFINDER_ENGINE` switch; `--version`/`--explain` report the engine in use
- [x] T-016 (P-058) CI matrix: differential and conformance on `native` and on `jaq`
- [x] T-017 (P-059) `tests/jq-suite/FLOOR-native`; the conformance runner selects it for the native engine
- [x] T-018 (P-060) `bench/`: startup (`-n .`), 30 MB single-line and line-delimited input, a filter-heavy set, an output-heavy set; record jq 1.8.1, jaq 3.1.1 and native figures
- [x] T-019 (P-061) `cargo fuzz` targets: `jqargs::parse`, `syntax::parse`, and evaluation of fuzzed filters over fuzzed input with a step limit

## M20 — jq's semantics in the engine

Each move: pin the behaviour with differential cases measured on jq 1.8.1
(several repairs have none today), give the engine the behaviour, switch the
repair off for the native engine (`Rewrites::NATIVE`, `prelude::NATIVE`),
and keep it for the jaq engine until P-078.

- [x] T-022 (P-062) Engine errors in jq's wording (indexing, iteration, arithmetic, types); the `catch_wording` rewrite and `_pf_err` off for native
- [x] T-023 (P-062) Builtins whose jq error names them (`startswith`, `endswith`, `ltrimstr`, `test`, `implode`, …) raise jq's error natively
- [x] T-024 (P-063) Assignment creates missing containers and raises jq's errors natively; the `assignment` rewrite, `_pf_vivify` and the `setpath` repair off for native
- [x] T-025 (P-064) `del`, `delpaths` and `L |= empty` keep jq's order and semantics natively; the `delete` rewrite and `_pf_delpaths` off for native
- [x] T-026 (P-065) Fractional indices, `null` and fractional slices, operand order, `/` and `%`, unary minus as jq has them; their rewrites off for native
- [x] T-027 (P-066) The engine's builtins are jq 1.8.1's: names jq lacks removed, the polyfilled ones defined natively (the 28 engine-only names are refused as jq refuses them; the polyfills remain)
- [x] T-029 (P-065) `def f($a): …` binds `a` as a filter too, as jq's desugaring `def f(a): a as $a | …` does; jaq leaves `a` undefined
- [x] T-028 (P-067) The native engine's prelude holds only definitions jq's `builtin.jq` has, each justified; `input_filename` from the driver as jq answers it (done: `input_filename`)
- [ ] T-030 jq's internal builtins a filter may call (`_plus/2`, `_modify/2`, `_negate/0`, `_sort_by_impl/1`, …; `check::JQ_INTERNAL`) defined in the engine, which reports them undefined (optional)

## M21 — One parser

- [x] T-031 (P-068) jaq's loader takes the main module's parser as a parameter (`jaq::Cli`); jaq's parser stays the fallback for what Pathfinder's passes through
- [x] T-032 (P-068) Pathfinder's syntax tree converts to the engine's terms for every construct, as slices of the program text so positions stay true; the native engine compiles from it, and the suites pass unchanged
- [x] T-033 (P-068) The rewrites that only steer jaq's parser (a leading `-`, `.[a].[b]`, …) are off for the native engine
- [x] T-034 (P-069) Syntax errors are reported as jq reports them: `syntax error, unexpected …` with jq's position and exit 3
- [ ] T-035 (P-070) `import`/`include` modules parsed by Pathfinder's parser too (optional)

## M22 — Regular expressions

jq 1.8.1's release binary runs its vendored Oniguruma 6.9.10 with
`ONIG_SYNTAX_PERL_NG`, UTF-8 and `ONIG_OPTION_CAPTURE_GROUP` (`f_match` in
`src/builtin.c`). The engine gets a matcher of its own that behaves as that
one does, its Unicode tables generated from Oniguruma's data files.

- [x] T-038 (P-071) `tools/onig-tables.py` writes Oniguruma 6.9.10's Unicode tables (properties, POSIX brackets, case folding) as Rust from jq's tarball; never edited
- [x] T-039 (P-071) A parser for `ONIG_SYNTAX_PERL_NG` as jq compiles it: escapes, classes and POSIX brackets, `\p{…}`, groups (named, non-capturing, atomic, lookaround, option `(?imx-imx)`, comment), quantifiers greedy/lazy/possessive and `{n,m}` with Oniguruma's literal fallback, backreferences; errors are Oniguruma's `Regex failure: …` text
- [x] T-040 (P-071) A backtracking matcher on byte positions with Oniguruma's stack discipline: its anchors (`Perl_NG`'s `^`/`$` are `\A`/`\Z`; `\z`, `\G`, `(?m)` lines), Unicode `\b`, case folds unravelled as Oniguruma unravels them, lookaround, backreferences, atomic groups, possessive and nested quantifiers, empty-loop checks, `\K`, `\R`, `\X`, conditionals
- [x] T-041 (P-072) jq's `f_match` in the engine as `_match_impl/3`: flags as Oniguruma options (`m` lets `.` match a newline, `l` finds the longest, `n` finds no empty match), offsets in code points, the step past an empty match; jq 1.8.1's definitions of `match`, `test`, `capture`, `scan`, `splits`, `split`, `sub`, `gsub`
- [x] T-042 (P-072) The shim's regex repairs off on the native engine; `regex-bites` removed from the engine crates
- [x] T-043 (P-071) jq's `onig.test` and `manonig.test` at 100% on the native engine, a probe corpus measured against jq 1.8.1 in the differential suite, and a benchmark against the engine it replaces
- [ ] T-044 (P-071) What `pathfinder-regex` rejects or approximates where jq's Oniguruma does more: subexpression calls (`\g<n>`, `(?&name)`, `(?R)`), the absent operator `(?~…)` and callouts (`(*FAIL)`, `(?{…})`) are refused with an error of Pathfinder's own; a backreference's nest level (`\k<n+1>`) matters only with calls; the capture-aware empty check (`EMPTY_CHECK_END_MEMST`, for a group a later backreference names inside a loop that can match empty) is the plain one; and the retry limit counts this matcher's backtracks, not Oniguruma's instructions, with only the literal check of Oniguruma's search optimisation (optional)

## M23 — Numbers and the remaining gaps

- [x] T-037 (P-070) A program with a `module` directive takes the syntax rewrites: Pathfinder's parser rejects the directive, so `module {}; .5` reaches jaq's parser unrewritten and fails there, on both engines (measured: jq prints `0.5`)
- [x] T-036 (P-073) A literal prints as jq's decNumber prints it: `1.` is `1`, `1.0` stays `1.0` (measured: `[.5, 1., 1.e2, 00012]` is `[0.5,1,1E+2,12]` in jq, `1.` here); the input reader takes `1.`, `.5`, `-.5`, `1.e3` as jq's does, and a negated zero literal is `0`
- [x] T-045 (P-074) `input` past the last input raises `"break"`, as jq's `f_input` does, so `try input catch .` is `"break"` and an uncaught one exits 5
- [x] T-046 (P-074) jq's time builtins on the native engine are its C functions on glibc 2.39, the C library its release binary links (`pathfinder-std/src/ctime.rs`): `timegm`/`mktime` normalising fields as `__mktime_internal` does, `gmtime`/`localtime` with years past 9999, `strftime` and `strptime` transcribed from glibc with their flags, widths, `E`/`O` modifiers and copied conversions, jq's sentinels and messages; `todate`/`fromdate` are jq's definitions; the shim's time repairs are off on the native engine
- [x] T-047 (P-074) `tools/time-diff.py` runs random time calls through jq 1.8.1 and the shim under four zones; 0 differences in 100 000 cases, outside the two documented ones (glibc's daylight saving overflow past year 5 881 600, jq's abort in `strflocaltime`)

## M24 — Release

- [x] T-048 (P-078) One engine at run time: no `exec`, spawn or jaq lookup; `translate::plan` builds the engine's run options alone; `PATHFINDER_ENGINE` other than `native` exits 2; `--version`, `--explain`, `--describe` report the one engine
- [x] T-049 (P-078) The jaq engine's rewrites and prelude gone: only the update of several paths (`_pf_modify`), unknown formats, `?//` and the syntax rewrites for jaq's parser remain; `src/regex.rs` removed
- [x] T-050 (P-078) `packaging/jaq/` and its patches removed; the Nix package builds one binary; `pathfinder-jaq` a library only; CI one suites job; one conformance ratchet, `FLOOR` (820)
- [x] T-051 `mjq`, a short name for `pathfinder`, installed by the Nix, Guix and Arch packages and `make install`
- [x] T-052 (P-080) README, AGENTS.md, SKILL.md, the Texinfo manual, `crates/UPSTREAM.md` and `doc/DIVERGENCES.md` describe one engine; every divergence re-measured
- [x] T-053 (P-079) Benchmarks against jq 1.8.1 and jaq 3.1.2 recorded in `doc/DIVERGENCES.md`
- [ ] T-054 (P-079) 24 h of fuzzing clean, all four targets: `regex` from 2026-10-31T04:11Z, `syntax` from 14:24Z (after T-057, T-058) and `jqargs` from 15:02Z ran clean; `engine` runs again from 2026-11-01T17:10Z (after T-059)
- [x] T-055 (P-081) Version 0.2.0
- [ ] T-056 (P-081) Bravais on Pathfinder 0.2.0 (`nix flake update pathfinder`), then the `v0.2.0` tag
- [x] T-057 (P-079, fuzzing) `range/1` and `range/2` take only numbers, as jq's `RANGE` opcode: `range("")` counted up for ever; a NaN bound counts for ever, as in jq; `transpose` is jq's definition
- [x] T-058 (P-079, fuzzing) The body of a `?//` chain is written once, as a definition: pasted into each alternative it grew threefold per link (12 links, 2 GB)
- [x] T-059 (P-079, fuzzing) A string's slice bound in (-1, 0) is its end, as in jq: `"abcd"[:-0.99]` was `""` (an overflow, a panic in a checked build)

## Standing

- [ ] T-020 Review jaq's releases after the import quarterly; port fixes by hand, never send anything upstream (§6.4)
- [ ] T-021 Keep `doc/DIVERGENCES.md` true at every milestone; a closed divergence moves to the repaired table with its measurement
