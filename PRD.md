<!--
SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
SPDX-License-Identifier: CC-BY-SA-4.0
-->

# Pathfinder — PRD

MVP: M18–M20 — Pathfinder engine v0.2: jaq's engine in-tree, in-process, at parity

**Status:** draft for the hard fork, 2026-10-11. **Baseline:** jq 1.8.1.
**Starting point:** jaq 3.1.1 with Pathfinder's patch set (`packaging/jaq/`).

## 1. Problem

Pathfinder is a shim: it translates jq's command line, rewrites the filter
where jaq would run it differently, and execs jaq. Steps 1–3 took jq 1.8.1's
own test suite from 77% (stock jaq) to 99.4% (815/820) on a patched jaq, and
the rest is out of a shim's reach:

- **The rewriter can only add text.** Every repair costs filter-level work at
  run time (`_pf_err` message translation, assignment vivification, regex
  result repair), and what cannot be expressed in jq source cannot be fixed.
- **The patch set is capped.** Patches must leave jaq's `Cargo.toml` and
  `Cargo.lock` alone so the Nix build can reuse upstream's vendored
  dependencies. That rules out the two remaining structural gaps: a regex
  engine with lookaround and Unicode `\b`, and decimal numbers for literals
  such as `1E+1000`.
- **Two programs, two error models.** jq's wording reaches scripts through a
  translation layer that parses jaq's messages back; path-expression errors
  cannot be translated at all, because jaq never tracks the path.
- **Every call pays for two processes' worth of startup** on the spawn path
  (`-a`, `--seq`, several files), and the shim cannot observe the engine's
  exit status or stderr on the exec path.

## 2. Goal

Pathfinder owns its engine. jaq's evaluator — fast, memory-safe, MIT — is
imported into this repository as Pathfinder's own crates, at the patched
3.1.1 state, and changed directly wherever jq 1.8.1 behaves differently.
Pathfinder becomes one binary that parses jq's command line and filter,
evaluates in-process, and answers as jq 1.8.1 does.

## 3. Users

- **The daily driver.** `jq` on Bravais resolves to Pathfinder. Scripts,
  shell pipelines and editors call it as jq and must not be able to tell.
- **Agents.** Coding agents call `jq` constantly; they read its stderr and
  exit codes. The native `pathfinder` surface (CLI Standard) stays as is.

## 4. Requirements

Priority: **mandatory** blocks the MVP; **expected** is in scope for the
fork; **optional** is welcome but never gates a milestone.

### Compatibility

- R-001 (mandatory) Under the name `jq`, stdout, stderr and exit status are
  jq 1.8.1's, byte for byte, on every differential case.
- R-002 (mandatory) jq 1.8.1's own test suite: no regression at any
  milestone (ratchet), and at least 815/820 when the engine goes in-process.
- R-003 (expected) At least 818/820 at the end of the fork: the regex and
  number milestones (M22, M23) own four of the five cases left.
- R-004 (mandatory) Never more permissive than jq: builtins and syntax jq
  1.8.1 does not define are removed from the engine, not kept as extras.
- R-005 (expected) jq's error wording is produced natively, including
  path-expression errors, retiring the `_pf_err` translation.
- R-006 (expected) Regular expressions behave as jq's Oniguruma does for the
  constructs in jq's suite: lookaround, Unicode `\b`, named groups.
- R-007 (expected) Number literals keep their decimal text until computed on,
  as jq's decNumber does (`1E+1000`, `100000000000000000000`).
- R-008 (optional) `--stream`, `--stream-errors`, `input_line_number`,
  `$__loc__`, `getpath/paths` edge cases, `modulemeta`, `get_search_list`.

### Stability and performance (Standard §3)

- R-009 (mandatory) `unsafe_code = "forbid"` in every crate. jaq's one
  `unsafe` outside Windows code (the `mmap` in file loading) is replaced by a
  safe read; Windows console code is dropped with jaq's CLI.
- R-010 (mandatory) No panic on any input: the parser and evaluator are
  fuzzed (`cargo fuzz`) before the MVP closes.
- R-011 (mandatory) Performance budget: on the benchmark set (`bench/`),
  no slower than jq 1.8.1, and within 10% of jaq 3.1.1 at the fork point.
  Measured with cycle counts, before and after every optimisation.
- R-012 (expected) Concurrency is decided per workload and documented: the
  evaluator is serial by design (jq's semantics are sequential over inputs);
  parsing large inputs ahead of evaluation is the candidate for a reader
  thread, adopted only if it measures faster.

### Packaging and process

- R-013 (mandatory) One binary, `pathfinder`, with the `jq` symlink; no
  runtime dependency on a jaq binary once the MVP closes.
- R-014 (mandatory) During migration both engines are selectable
  (`PATHFINDER_ENGINE=native|jaq`), and CI runs the differential and
  conformance suites against both until the jaq path is retired.
- R-015 (mandatory) Every new third-party crate passes `cargo audit` and is
  justified in the PR that adds it (§3.3).
- R-016 (mandatory) Licensing: imported jaq files keep MIT and their
  upstream copyright (§4.2), are listed in `REUSE.toml`, and credited in
  `CREDITS.md` (§15.3). Pathfinder's changes to them, and its own files
  among the engine crates, are GPL-3.0-or-later OR MIT, so each engine file
  stays usable under MIT; the shim crate stays GPL-3.0-or-later.
- R-017 (mandatory) Nothing is sent upstream (§6.4). Upstream jaq fixes are
  reviewed and ported by hand; the import records its base commit.

## 5. Non-goals

- jaq's extra input and output formats (YAML, TOML, CBOR, XML, CSV/TSV
  reading) and its `-i/--in-place`: jq has none of them. They are dropped
  from the import, which also drops their dependencies.
- jaq's own CLI, REPL and playground (`jaq/`, `jaq-play`).
- A new jq dialect. Behaviour that differs from jq 1.8.1 is a bug, however
  attractive.
- The native `pathfinder` surface changes only as the CLI Standard requires.

## 6. Architecture

```
crates/
  pathfinder/          bin: jq argv, native surface, program assembly (today's src/)
  pathfinder-core/     from jaq-core: compiler, evaluator, paths, exceptions
  pathfinder-std/      from jaq-std: builtins, regex, time, math
  pathfinder-json/     from jaq-json: values, numbers, reader, writer
```

- **Import, then diverge.** M18 brings the crates in unchanged apart from the
  patch set and renames, so the first diff against upstream is reviewable.
  Every later change is a normal Pathfinder commit.
- **One parser, eventually.** Pathfinder already parses jq's grammar
  (`src/syntax/`); jaq parses its own dialect. Until M21 the engine keeps
  jaq's loader and the rewriter keeps feeding it; M21 compiles from
  Pathfinder's syntax tree, which retires the rewriter's text splicing.
- **Repairs move down a layer.** Each prelude polyfill or rewrite becomes an
  engine change when its milestone comes, guarded by the differential case
  that already pins it; the polyfill is deleted in the same change.
- **The jaq path is kept until it is redundant**, then removed (M24).

## 7. Risks

| Risk | Mitigation |
|---|---|
| Losing jaq's performance while changing semantics | Budget R-011 checked per PR; benchmarks before and after each change |
| Upstream fixes are missed | Record the base commit; review jaq's releases each quarter (TODO) |
| Regex fidelity needs Oniguruma itself | Prefer a pure-Rust engine (`fancy-regex`); a C binding is a §3.1 exemption and needs ASLR/CFI and a filed justification |
| Decimal numbers slow arithmetic | Keep literals as text until first computed on, as jq does; measure |
| The fork stalls half-way | MVP is parity, not perfection: in-process at 815/820 already pays for itself |

## 8. Success metrics

- jq 1.8.1 suite: 815/820 at the MVP, 818/820 or better at M23.
- Differential suite (stdout, stderr, exit status): 100% on both engines
  until the jaq path is retired.
- Benchmarks: R-011 holds at every milestone.
- `cargo fuzz` runs clean for 24 h on parser and evaluator before release.
