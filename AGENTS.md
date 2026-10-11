# AGENTS.md — Pathfinder

Project-specific invariants for AI coding agents. General Rust and CLI rules come
from the Spacecraft Software skills (listed in [`CLAUDE.md`](CLAUDE.md)); this
file is only what is true *here*.

## What this is

A binary, `pathfinder`, installed with a `jq` symlink beside it. It
translates jq's command line into jaq's, rewrites the filter where jaq would run
it differently, and hands off. It parses jq (`src/syntax/`) but evaluates
nothing.

The repository is a Cargo workspace. The shim is `crates/pathfinder`; paths
like `src/…` and `tests/…` in this file are relative to it. The root
`Cargo.toml` holds the lint set and the release profile, and its
`default-members` keeps a bare `cargo build`/`test`/`clippy` on the shim;
`--workspace` reaches every crate.

Baseline is **jq 1.8.1**. The differential suite passes unchanged against jaq
**3.0.0, 3.1.0 and 3.1.1**; CI pins 3.1.0, the version nixpkgs 26.05 ships and
therefore the one daily-driven through Bravais. jaq 3.1 added `input_filename`
natively; Pathfinder's own definition still takes precedence, deliberately,
because it answers jq's way across concatenated multi-file input.

## Build, test, lint

```sh
cargo build --release
cargo test                       # unit + differential
cargo clippy --all-targets -- -D warnings
cargo fmt --check
make check                       # all of the above
```

The engine crates build with the workspace (`cargo build --workspace`).
`crates/pathfinder-jaq` builds a `jaq`-compatible binary from them, the test
vehicle for the fork until M19 evaluates in process; run the suites against it
as the engine:

```sh
cargo build --release -p pathfinder-jaq
PATHFINDER_JAQ=$PWD/target/release/pathfinder-jaq cargo test
```

## The rule that matters most

**Never claim a jq behaviour you have not measured.** Real jq is not installed on
the dev host; get one ephemerally and compare:

```sh
JQ=$(nix build --no-link --print-out-paths nixpkgs#jq | head -1)/bin/jq
printf '{}' | $JQ  -c '.a.b = 1'
printf '{}' | jaq -c '.a.b = 1'
```

Four beliefs that felt obvious turned out to be wrong when checked: `--indent 0`
does *not* mean compact, malformed-JSON input exits 5 in both (not 2 in jq),
`ltrimstr` on a non-string errors in jq 1.8.1 too, and `leaf_paths`/`ascii`/
`ANY`/`ALL`/`toarray` are not jq builtins at all. Every one of those would have
shipped as a bug.

Any behavioural change belongs in `tests/differential.rs` as a case, not in a
hand-written golden.

## The CLI Standard applies to the native name only

Under the name `jq` the command line, output, exit codes and diagnostics are
jq 1.8.1's — that compatibility is the product, and the Spacecraft CLI
Standard's global flags (`--json`, `--format`, `--quiet`, …) would collide
with jq's own. Agent detection (`AI_AGENT`, `AGENT`, `CI`) must not change
anything a `jq` call prints; the differential suite runs under a live agent
environment and proves it.

The Standard governs the native `pathfinder` surface: `--explain`,
`--version` and `--install-shim`. Each reports as text or as one compact JSON
envelope (`{"metadata":…,"data":…}`), chosen by the cascade in `src/mode.rs`:
`--json`, then a non-empty `AI_AGENT`/`AGENT`, then a truthy `CI`, then a
terminal on stdout means text, else JSON. Every failure of a report in JSON
mode — usage, missing filter, compile error, unsupported feature, install
failure — goes to stderr as one `{"error":…}` line, with a runnable `hint`
when one is known (never a guessed one); compile errors keep jq's exit 3,
`--install-shim` exits 5 on a conflict and 4 on a permission failure. The
route is `Lifted::json_report`, which is `None` under the `jq` name, so
`diag::die` and `error.render` stay the only failure text a `jq` call can
print. To keep the `jq` name untouched:

- `--json` is lifted out of argv only under the native name and only beside a
  native verb or `--version`/`-V`; anywhere else it reaches the jq parser and
  is rejected as jq rejects it (exit 2). Flags and verbs count only in option
  position (`jqargs::option_positions`): an option's value (`--arg x --json`)
  or a word after `--` is the user's data and is never lifted.
- The mode is decided inside the native branches, so a `jq` call never reads
  the agent variables. `CLAUDECODE`/`CURSOR_AGENT`/`GEMINI_CLI` only label
  `metadata.invoking_agent`.
- `jq --version` stays `jq-1.8.1`, and `jq --help` stays jq's text; the
  native `--help` adds a trailer on stderr.
- `--version` reports the jaq lookup without spawning jaq; the lookup order
  lives once, in `main.rs`'s `jaq_binary`.
- No runtime dependency for this: `src/json.rs` writes the JSON. `serde_json`
  is a dev-dependency, used by `tests/native.rs` to parse the output.

## Architectural invariants

- **Whitelist, never pass through.** Every flag reaching jaq is named explicitly
  in `translate.rs`. This is a *safety* property, not tidiness: jaq has
  `-i/--in-place` and jq does not, so forwarding unknown flags would turn
  `jq -i '.' data.json` into a silent, irreversible file rewrite.
- **Do not become more permissive than jq.** A name missing from jaq is polyfilled
  only if jq 1.8.1 actually defines it. Accepting a filter real jq rejects lets a
  script pass here and fail on the next machine — the worst outcome for a shim.
- **The `exec` path must stay the default.** Give it up only for a transform that
  genuinely cannot be expressed in jaq's argv (`-a`, `--seq`, multi-file input).
- **The filter stays byte-identical unless something is needed.** `assemble`
  returns `rewritten: false` in that case and `-f` is kept, preserving jaq's error
  provenance.
- **Prelude on one line.** jaq reports errors against the text it was handed; a
  prelude on its own line would shift every user line number by one.
- **A repair reaches the original through an alias defined before it.**
  Calling the shadowed name inside its own shadow recurses forever under jaq.
  But definitions bind names where they are written, so
  `def _pf_mktime: mktime; def mktime: … | _pf_mktime;` reaches the builtin.
  Emit the alias and the shadow together, alias first; when one repair uses
  another (`todateiso8601` → `strftime`), declare it in `deps` so emission
  order makes it bind the repair, not the builtin.
- **jaq's two-argument regex builtins reject `null` flags**; jq treats `null`
  as none. Route the no-flags case to jaq's one-argument builtin.
- **jaq reads any leading `-` as a flag**, where jq's `isoptish` does not. A
  program starting with `-` gets a leading space; a file `-x` becomes `./-x`; a
  `--args` value starting with `-` is bound by name. Never use `--` to fix this:
  after `--`, jaq reads `-` as a file literally named `-` instead of stdin.
- **No wrapper script in front of the binary.** The Nix package pins jaq by
  compiling its store path in (`PATHFINDER_DEFAULT_JAQ` → `PINNED_JAQ`), not with
  `wrapProgram`, which would fork a shell before every `jq` call. Lookup order is
  `PATHFINDER_JAQ`, then the pin, then `jaq` on `PATH`.
- **stderr is inherited, never captured**, so jaq's diagnostics, `debug` output,
  colour detection and interleaving stay correct.
- **Several input files reach jaq as one stdin stream, described in
  `PATHFINDER_INPUTS`** (`src/inputs.rs`; read by `jaq/src/jq.rs`, patch
  `0015`): names, byte counts, and jq's message for an unreadable file. Nothing
  goes between files, because jq inserts nothing. The packaged jaq makes each
  boundary an `fgets` chunk boundary, names the file being read and restarts
  its line count; a stock jaq ignores the variable. The two sides share one
  record format (`<bytes>:<n>:<name><m>:<message>`): change both or neither.

### The parser and the rewriter (`src/syntax/`)

- **The grammar is jq's.** `parse.rs` mirrors `src/parser.y`'s rules and
  precedence, `lex.rs` mirrors `src/lexer.l`. Validate a grouping change with
  `PATHFINDER_DEBUG_REPRINT=1 make conformance`, which runs the whole suite
  through the fully parenthesised printer.
- **Splice, never reprint.** Only rewritten nodes are re-emitted; every other
  byte is copied from the source. A rewritten node is always parenthesised.
- **A parse failure passes the filter through** — it may be a gap in this
  parser. Only a construct *proven* to be a jq compile error exits 3: a constant
  non-string object key (by jq's own folding rules, `check.rs`), non-constant or
  non-object `module` metadata, and a parse failure *at* a `?//` token.
- **Assignment is vivify-then-native.** Prepare the containers (inline guard for
  a literal target, `_pf_vivify` otherwise), then let jaq's own operator run —
  it matches jq once the containers exist. jq's `_modify` transcribed into jaq
  is quadratic (2.4 s for 10k elements); a comma target is the only thing still
  sent through `_pf_modify`.
- **`del(f)` uses jaq's `del` only where it is jq's**: one path per array
  (`single_path`), a deleting step that is not a named key (jaq swap-removes
  keys, breaking order), with `try … catch` falling back to `_pf_delpaths` on
  the original input. The fallback evaluates the target twice, so effectful
  calls (`input`, `debug`, …) keep a target off this route (`repeatable`).
- **Program-defined names disable reasoning about them.** If the filter defines
  `del`, `select`, or a name `single_valued` trusts, the `del` route is off.
- **Emit `.[a][b]`, never `.[a].[b]`** — jaq 3.0 does not parse the latter.
  The user's own `.a.[0]`/`.a.[]` are spliced without the dot for the same
  reason (`splice_mapping`).
- **Index keys and slice bounds are evaluated against the input, not the
  base.** `[1,2][0:.]` slices with the outer `.`; a rewrite that pipes the base
  first must bind the bounds before it. A non-literal key is rounded inline
  (`INDEX_NORM`) so it stays a path expression; never guard with `type ==`.
- **`src/regex.rs` and the prelude's `_pf_scanre` must name groups
  identically.** The Rust side precomputes literal regexes, the jq side scans
  computed ones; the two are transcriptions of one tokenising regex. Change
  both or neither. The literal table is *bound*, not defined
  (`{…} as $__pf_rl | def …`): a definition rebuilds it on every call.
- **Measure the cost of a jq-level definition before adding one per element.**
  A filter-parameter call, `first`, `type` (about 4 µs under jaq 3.1) and
  `and`/`or` (several µs each) are expensive; comparisons (`. < ""` is "not a
  string, number or later type"), nested `if`s, inline text and `label`/`break`
  are cheap. Type tests in a per-element path are written as comparisons.
- **Error text reaches scripts only through `catch`.** The rewriter wraps a
  handler that reads its input in `_pf_err`, which turns jaq's templates back
  into jq's wording; a builtin whose jq message names it gets a guard that runs
  jaq's builtin first and re-raises with jq's text only on a wrong input type.
- **The packaged jaq words an uncaught error by the same rules.**
  `packaging/jaq/0008-uncaught-errors.patch` adds `jaq/src/jq.rs`, whose
  `wording` is a Rust transcription of `_pf_err` (and its `dump` of
  `_pf_dump`). Change both or neither, then regenerate the patch from the jaq
  source tree; `patched_engine_reports_errors_as_jq` compares stderr and exit
  status with real jq.
- **The error location counts lines the way jq reads them**: `fgets` into a
  4096-byte buffer, so up to a newline or 4095 bytes at a time, and through
  the chunk holding the last byte the parser needed (the delimiter after a
  number or literal, the closing character otherwise). On
  stdin `jq::Lines` is the only buffer and serves single bytes without a call;
  a reader layered under std's `Read::bytes` cost up to 66% on one long line.
  Measure stdin with `perf stat -e cycles:u`, one run per invocation (`-r`
  gives later runs an empty stdin).
- **A parse error is worded by jq's own parser, transcribed.**
  `jaq-json/src/parse_error.rs` (patch `0013`) is `jv_parse.c`'s state
  machine without the values; jaq's parser builds the values, and on failure
  the value that failed is re-scanned from jq's initial state. On stdin
  `jq::Position` implements `Recall`, keeping that value's text only when the
  line buffer is compacted. jaq's reader rejects what jq's rejects (comments,
  byte strings, non-string keys, run-on literals) so that both stop at the
  same byte; loosening it makes the two disagree on where an error is.
- **A path expression fails where jq's does** (patch `0010`): a non-path
  filter in path mode yields a *lost* path (`Node::Lost`), which fails only
  when a later step uses it. The messages start `Invalid`, which `_pf_err`
  and `jq.rs::wording` pass through unchanged; keep it so.

## Conformance

`tests/conformance.rs` runs jq 1.8.1's own test suite (`tests/jq-suite/`,
vendored, MIT) through the shim — the coverage measure, as opposed to the
differential suite's statement of intent. `make conformance` prints per-file
pass rates; `V=1` lists every failure.

- `FLOOR` is a ratchet. Raise it in the change that raises the score; never
  lower it to make a change pass. `FLOOR-patched` is the same ratchet for the
  packaged, patched jaq; the test picks it when the engine's `--version`
  contains `+pathfinder`.
- **The patches are developed against jaq's source, not edited by hand.**
  Unpack the pinned jaq's `src`, commit it, make one commit per patch, and
  regenerate `packaging/jaq/*.patch` with `git diff`. Never touch jaq's
  `Cargo.toml`/`Cargo.lock`: the Nix build reuses the upstream vendored
  dependencies.
- `EXCLUDED` is generated from real jq 1.8.1, never hand-edited.
- A rising total can hide regressions: compare the failing-case lists before and
  after (`V=1`), not just the count. The first `match`/`test`/`capture` repair
  raised the total while breaking nine cases, two of them real call sites.

## Layout

| File | Owns |
|---|---|
| `Cargo.toml` | The workspace: members, `default-members`, the shared lint set (`[workspace.lints]`) and the release profile. One `Cargo.lock`, at the root. |
| `crates/pathfinder/src/jqargs.rs` | jq's argv grammar, transcribed from jq's `src/main.c`. Hand-rolled; clap cannot express it. |
| `crates/pathfinder/src/scan.rs` | The jq token scanner — the crate's only real algorithm. Strings, `\(…)` interpolation nesting, comments, module headers. |
| `crates/pathfinder/src/prelude.rs` | Polyfill and repair definitions, and the evidence for each. |
| `crates/pathfinder/src/program.rs` | Program assembly: module header, prelude, `$ARGS` wrap, `$__loc__`. |
| `crates/pathfinder/src/translate.rs` | jq argv → jaq argv. The only module that knows both dialects. |
| `crates/pathfinder/src/post.rs` | `-a` and `--seq` byte transforms. |
| `crates/pathfinder/src/inputs.rs` | Several input files: opened before jaq starts, described to it in `PATHFINDER_INPUTS`, fed as one stream. |
| `crates/pathfinder/src/native.rs` | `--explain` / `--version` / `--install-shim`, their text and JSON reports, and structured errors; reachable only under the native name. |
| `crates/pathfinder/src/mode.rs` | The native verbs' output-mode cascade (CLI Standard §5). |
| `crates/pathfinder/src/json.rs` | Minimal JSON writer (RFC 8259 escaping) for the native reports; std-only. |
| `crates/pathfinder/src/syntax/lex.rs`, `parse.rs` | jq's lexer and grammar, transcribed. Every node keeps its span. |
| `crates/pathfinder/src/regex.rs` | Facts about literal regexes for the regex repairs: group naming (mirrors the prelude's run-time scan), whether a group can be skipped, whether the regex can match empty. |
| `crates/pathfinder/src/syntax/rewrite.rs` | Source-to-source rewrites: assignment, `del`, `?//`, `{$b: p}`, computed-key checks, compound `reduce` sources. |
| `crates/pathfinder/src/syntax/check.rs` | jq's compile-time rejections, and jq's constant folding to decide them. |
| `crates/pathfinder/src/syntax/print.rs` | Fully parenthesised printer: parser validation, and pattern text for rewrites. |
| `packaging/jaq/` | Patches to jaq's own source (number output, arithmetic precision, input literals, string repetition, object keys, `debug`, uncaught errors, `halt_error`, path expressions, `l // r` as a path, `%`, parse errors, lazy input reading, file names in error locations), applied by `packaging/default.nix`. Cut against 3.1.1 (this flake's nixpkgs); verified to apply and pass on 3.1.0, which Bravais builds by making this flake follow its stable nixpkgs. `0008`, `0013` and `0015` have a 3.1.0 cut, picked by version, because 3.1.0's `real_main` differs on the lines they replace. MIT, like jaq; never sent upstream. |
| `crates/pathfinder-core`, `-std`, `-json`, `-fmts` | jaq 3.1.1's engine crates, imported with the patch set applied (`crates/UPSTREAM.md`). Library names stay `jaq_core`, … so the sources stay diffable against upstream. |
| `crates/pathfinder-jaq` | jaq 3.1.1's CLI and `jaq-all` on those crates, with the CLI hunks of the patch set: the binary `pathfinder-jaq`, a test vehicle run as `PATHFINDER_JAQ` until M19. `publish = false`; never installed. |
| `crates/pathfinder/tests/differential.rs` | The compatibility contract: byte-exact agreement with real jq on chosen cases. |
| `crates/pathfinder/tests/conformance.rs` | The coverage measure: jq's own suite, judged as jq's runner judges it. |
| `crates/pathfinder/tests/native.rs` | The native surface: mode cascade, JSON shapes, exit codes, and that the `jq` name is unchanged under an agent environment. |
| `SKILL.md` | Agent-facing description of the shim. Frontmatter first, so its licence is in `REUSE.toml`; keep `description` ≤ 1000 rendered characters. |

## Forbidden patterns

- `unsafe` — the crate sets `unsafe_code = "forbid"`.
- Forwarding an unrecognised flag to jaq.
- Polyfilling a name jq 1.8.1 does not define.
- Adding a runtime dependency to the jq path; it is std-only and stays that way.
- A golden value in a test that was not produced by running real jq.

## Gotchas

- `prelude::render` returning `""` is the *good* case: it means the fast path is
  available.
- `$ARGS.named` must not leak the `__pathfinder_*` bindings the `--jsonargs`
  emulation adds; the wrap filters them out.
- `$__loc__.file` is `"<top-level>"` in jq for inline filters *and* `-f` files.
- **Never edit a file you have not just read by blind string replacement.** This
  file was committed empty in the very first commit and every later
  replace-into-it was a silent no-op until it was noticed. Assert that the text
  you are replacing exists.
- jq's `builtins` is unsorted; the embedded copy must stay in jq's order or
  `builtins|sort == builtins` answers `true` where jq answers `false`.

## Standards

Every file carries the two SPDX tags (REUSE, Standard §4.3). Software is
`GPL-3.0-or-later`; docs are `CC-BY-SA-4.0`; the embedded jq help text keeps
upstream jq's MIT (§4.2). Commits to a Spacecraft Software remote must be signed
and show "Verified" (§6.3).
