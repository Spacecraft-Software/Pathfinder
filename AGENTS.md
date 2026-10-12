# AGENTS.md — Pathfinder

Project-specific invariants for AI coding agents. General Rust and CLI rules come
from the Spacecraft Software skills (listed in [`CLAUDE.md`](CLAUDE.md)); this
file is only what is true *here*.

## What this is

A binary, `pathfinder`, installed with a `jq` symlink beside it. It parses
jq's command line and filter (`src/jqargs.rs`, `src/syntax/`), rewrites the
filter where the engine would run it differently from jq, and evaluates it on
one of two engines (`src/engine.rs`, PRD R-014):

- **`native`** — Pathfinder's engine crates (a hard fork of jaq 3.1.1),
  linked in and run in process. The default of a plain `cargo build`.
- **`jaq`** — a jaq binary, `exec`ed or spawned. The default of the Nix
  package until the native engine replaces it (PLAN P-081).

`PATHFINDER_ENGINE=native|jaq` chooses at run time; unset, the build's default
applies (`PATHFINDER_DEFAULT_ENGINE` at build time, else `native`).

The repository is a Cargo workspace. The shim is `crates/pathfinder`; paths
like `src/…` and `tests/…` in this file are relative to it. The root
`Cargo.toml` holds the lint set and the release profile, and its
`default-members` keeps a bare `cargo build`/`test`/`clippy` on the shim;
`--workspace` reaches every crate.

Baseline is **jq 1.8.1**. The differential suite passes unchanged against jaq
**3.0.0, 3.1.0 and 3.1.1**; CI pins 3.1.0, the version nixpkgs 26.05 ships and
therefore the one daily-driven through Bravais. `input_filename` is jq's on
the native engine, answered from the driver's reader (`jq::Position`):
`null` before any input is read, then the file the reader has open, across
several files too. On the jaq engine the prelude defines a constant, which
can be only one of those (`null` under `-n`, else the input's name; an error
with several files).

## Build, test, lint

```sh
cargo build --release
cargo test                       # the shim: unit + differential
cargo test --workspace --exclude pathfinder-jq   # the engine crates
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
make check                       # all of the above
```

A bare `cargo test` runs the suites on the native engine. Run them on the jaq
engine too — CI runs both (`make test-jaq` takes `JAQ=<binary>`):

```sh
PATHFINDER_ENGINE=jaq PATHFINDER_JAQ=/path/to/jaq cargo test
```

`crates/pathfinder-jaq` also builds `pathfinder-jaq`, a `jaq`-compatible
binary on the engine crates, for checking that the in-tree crates and the
packaged jaq agree:

```sh
cargo build --release -p pathfinder-jaq
PATHFINDER_ENGINE=jaq PATHFINDER_JAQ=$PWD/target/release/pathfinder-jaq cargo test
```

## The rule that matters most

**Never claim a jq behaviour you have not measured.** Real jq is not installed on
the dev host; get one ephemerally and compare:

```sh
# jq 1.8.1's release binary, as CI fetches it (nixpkgs has moved to 1.8.2).
curl -fsSL -o jq https://github.com/jqlang/jq/releases/download/jq-1.8.1/jq-linux-amd64
echo "020468de7539ce70ef1bceaf7cde2e8c4f2ca6c3afb84642aabc5c97d9fc2a0d  jq" | sha256sum -c -
chmod +x jq
printf '{}' | ./jq -c '.a.b = 1'
printf '{}' | PATHFINDER_ENGINE=native target/release/pathfinder -c '.a.b = 1'
```

Point `PATHFINDER_REAL_JQ` at it for the suites: their fallback,
`nix build nixpkgs#jq`, now finds 1.8.2 and skips.

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
`--version`, `--install-shim`, `--describe` and `--schema`. The last two are
the Standard's `describe` and `schema` as flags, because a bare word is a jq
program; both render from one table (`src/describe.rs`), so a new native
option is added there or `--describe` and `--schema` miss it. `--schema`
prints raw JSON Schema in every mode (`--format` wraps it for an LLM
provider); every `parameters` schema has a `type` and a `description` on
each property, which `describe.rs`'s tests check. Each reports as text or as one compact JSON
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
- No third-party dependency for this: `src/json.rs` writes the JSON.
  `serde_json` is a dev-dependency, used by `tests/native.rs` to parse the
  output.
- `--version` and `--explain` name the engine (`engine`, and for the jaq
  engine its binary and lookup step); a `PATHFINDER_ENGINE` naming no engine
  fails under either name with exit 2, as a structured error in JSON mode.

## Architectural invariants

- **Whitelist, never pass through.** Every flag reaching jaq is named explicitly
  in `translate.rs`. This is a *safety* property, not tidiness: jaq has
  `-i/--in-place` and jq does not, so forwarding unknown flags would turn
  `jq -i '.' data.json` into a silent, irreversible file rewrite.
- **One plan, two renderings.** `translate::plan` decides everything once and
  renders it twice: `Plan::argv` for a jaq binary, `Plan::cli` (the engine's
  run options, `jaq::Cli`) for the native engine, built from jq's command
  line directly. `run_options_agree_with_jaqs_reading_of_the_argv` pins the
  two together; a new flag goes into both or the test fails.
- **The native engine is jaq's CLI as a library** (`crates/pathfinder-jaq`,
  lib `jaq`): `jaq::run` takes the run options, stdin, the concatenated
  files' `Segment`s and stdout, and returns the exit status. It never exits
  the process itself (`halt` included), so the caller flushes first. The
  shim around it does what it does around a jaq binary, in process: several
  files are one `inputs::reader` stream described by `inputs::segments`
  (never `PATHFINDER_INPUTS`, which would show in `$ENV`), `--seq` input goes
  through `post::StripRs`, and `-a`/`--seq` output through `post::Framer`,
  the same framer that reshapes a jaq binary's stdout.
- **Do not become more permissive than jq.** A name missing from jaq is polyfilled
  only if jq 1.8.1 actually defines it. Accepting a filter real jq rejects lets a
  script pass here and fail on the next machine — the worst outcome for a shim.
- **On the jaq engine, the `exec` path stays the default.** Give it up only for
  a transform that genuinely cannot be expressed in jaq's argv (`-a`, `--seq`,
  multi-file input).
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
  non-object `module` metadata, a parse failure *at* a `?//` token, and a
  reachable call to a name/arity that is neither one of jq 1.8.1's
  `builtins` (`prelude::JQ_BUILTINS`), a literal, a `_` name, nor defined by
  the program (`check::undefined_calls`). That covers jaq's own helpers
  (`isarray`, `tobytes`, `halt/1`, …), which a filter could otherwise call,
  and replaces jaq's report of an unknown name, which quoted the whole
  prelude. It follows jq's scoping: a definition counts only once something
  reachable calls it, arguments always count, and an `import`/`include` or a
  `~/.jq` file turns it off.
- **Depth is bounded, because every pass recurses on the native stack.**
  The lexer stops at `MAX_INTERP` nested interpolations, the parser at
  `MAX_NESTING` recursive calls, `MAX_FOLDS` chain links (`,`, binary
  operators, postfixes) in all, and a tree `MAX_DEPTH` deep. Past any of them
  the program is a parse failure and passes through; the error is at offset
  0, never at a `?//`. Unbounded, a few thousand levels aborted the shim.
  A program over 4 KiB is assembled on a 256 MiB thread (`program::assemble`):
  the bounds alone kept a release build inside 8 MiB but not CI's debug one. A
  new recursive pass over the tree must stay within `MAX_DEPTH`; a new
  grammar cycle must pass through `query`, `term` or `pattern`, or be wrapped
  in `Parser::nested` where it recurses (as `//` is in `expr`).
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
- **Error text reaches scripts only through `catch`.** The native engine
  words its errors as jq does (M20): the core's `Error::typ`/`math`/`index`
  and the builtins raise jq 1.8.1's messages, built with `ValT::jq_quoted`.
  On the jaq engine the rewriter wraps a handler that reads its input in
  `_pf_err`, which turns jaq's templates back into jq's wording, and a
  builtin whose jq message names it gets a guard that runs jaq's builtin
  first and re-raises with jq's text only on a wrong input type.
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
  lower it to make a change pass. `FLOOR-native` is the same ratchet for the
  native engine and `FLOOR-patched` for the packaged, patched jaq; the test
  asks `pathfinder --version --json` which engine runs, and a jaq whose
  `--version` contains `+pathfinder` is the patched one. A stock jaq is held
  to `FLOOR`.
- **The patches are developed against jaq's source, not edited by hand.**
  Unpack the pinned jaq's `src`, commit it, make one commit per patch, and
  regenerate `packaging/jaq/*.patch` with `git diff`. Never touch jaq's
  `Cargo.toml`/`Cargo.lock`: the Nix build reuses the upstream vendored
  dependencies.
- **Until `packaging/jaq/` is retired (P-078), an engine fix lands twice**:
  as a commit in jaq's source tree, folded into the patch it repairs and the
  patches regenerated, and as the same diff applied to `crates/pathfinder-*`.
  The packaged jaq and the in-tree crates must stay identical in behaviour;
  `pathfinder-jaq` run as `PATHFINDER_JAQ` checks it.
- `EXCLUDED` is generated from real jq 1.8.1, never hand-edited.
- A rising total can hide regressions: compare the failing-case lists before and
  after (`V=1`), not just the count. The first `match`/`test`/`capture` repair
  raised the total while breaking nine cases, two of them real call sites.

## Fuzzing

`fuzz/` is a `cargo fuzz` crate with its own Cargo root, outside the
workspace. It has no lib target to link against, so `fuzz/src/lib.rs`
includes the shim's modules by `#[path]` (`jqargs`, `scan`, `regex`,
`prelude`, `program`, `syntax`); a new `crate::` dependency of those needs a
line there. Three targets:

| Target | Input | `-max_len` |
|---|---|---|
| `jqargs` | argv, NUL-separated, any bytes | 512 |
| `syntax` | a jq program: lex, parse, print, `program::assemble` | 4096 |
| `engine` | `filter NUL json`: assembled, compiled, run through the engine crates | 1024 |

```sh
cd fuzz
env -u CFLAGS -u CXXFLAGS cargo fuzz run -s none syntax corpus/syntax seeds/syntax -- \
  -fork=2 -ignore_crashes=1 -ignore_timeouts=1 -ignore_ooms=1 \
  -max_total_time=300 -timeout=10 -rss_limit_mb=2048 -max_len=4096
```

- `-s none` builds on stable: everything is `unsafe`-free, so a sanitizer
  adds nothing, and only `-Zsanitizer` needs a nightly. `env -u CFLAGS` keeps
  the host's `-flto=auto` out of libFuzzer's C++ build.
- `engine` skips filters that can run without end or end the process
  (`def`, `repeat`, `while`, `until`, `recurse(`, `halt`, `debug`, `stderr`,
  numbers of four digits or with an exponent) and caps inputs and outputs;
  what remains unbounded is cut off by `-timeout`/`-rss_limit_mb`, so a
  timeout or OOM there is not a finding without a bounded reproducer. Its
  first finding, a panic on `[1 #`, is fixed (patch `0016`).
- A crash is reproduced with `cargo fuzz run -s none <target> <artifact>`.
  A shim crash gets a fix and a unit test; an engine crash lands twice (see
  Conformance).
- `fuzz/seeds/` is committed (jq's suite for `syntax`/`engine`); `corpus/`,
  `artifacts/` and `target/` are not.

## Layout

| File | Owns |
|---|---|
| `Cargo.toml` | The workspace: members, `default-members`, the shared lint set (`[workspace.lints]`) and the release profile. One `Cargo.lock`, at the root. |
| `crates/pathfinder/src/jqargs.rs` | jq's argv grammar, transcribed from jq's `src/main.c`. Hand-rolled; clap cannot express it. |
| `crates/pathfinder/src/scan.rs` | The jq token scanner — the crate's only real algorithm. Strings, `\(…)` interpolation nesting, comments, module headers. |
| `crates/pathfinder/src/prelude.rs` | Polyfill and repair definitions, and the evidence for each. |
| `crates/pathfinder/src/program.rs` | Program assembly: module header, prelude, `$ARGS` wrap, `$__loc__`. |
| `crates/pathfinder/src/translate.rs` | jq argv → jaq argv. The only module that knows both dialects. |
| `crates/pathfinder/src/engine.rs` | The engine switch (`PATHFINDER_ENGINE`) and running the native engine in process. |
| `crates/pathfinder/src/post.rs` | `-a` and `--seq` byte transforms: `Framer` (output) and `StripRs` (input), for both engines. |
| `crates/pathfinder/src/inputs.rs` | Several input files: opened before the engine starts, read as one stream, and described to it — as `Segment`s in process, in `PATHFINDER_INPUTS` to a jaq binary. |
| `crates/pathfinder/src/native.rs` | `--explain` / `--version` / `--install-shim`, their text and JSON reports, and structured errors; reachable only under the native name. |
| `crates/pathfinder/src/describe.rs` | `--describe` and `--schema`: the native surface described from one table of commands. |
| `crates/pathfinder/src/mode.rs` | The native verbs' output-mode cascade (CLI Standard §5). |
| `crates/pathfinder/src/json.rs` | Minimal JSON writer (RFC 8259 escaping) for the native reports; std-only. |
| `crates/pathfinder/src/syntax/lex.rs`, `parse.rs` | jq's lexer and grammar, transcribed. Every node keeps its span. |
| `crates/pathfinder/src/regex.rs` | Facts about literal regexes for the regex repairs: group naming (mirrors the prelude's run-time scan), whether a group can be skipped, whether the regex can match empty. |
| `crates/pathfinder/src/syntax/rewrite.rs` | Source-to-source rewrites: assignment, `del`, `?//`, `{$b: p}`, computed-key checks, compound `reduce` sources. |
| `crates/pathfinder/src/syntax/check.rs` | jq's compile-time rejections, and jq's constant folding to decide them. |
| `crates/pathfinder/src/syntax/print.rs` | Fully parenthesised printer: parser validation, and pattern text for rewrites. |
| `packaging/jaq/` | Patches to jaq's own source (number output, arithmetic precision, input literals, string repetition, object keys, `debug`, uncaught errors, `halt_error`, path expressions, `l // r` as a path, `%`, parse errors, lazy input reading, file names in error locations, a comment at the end of the program), applied by `packaging/default.nix`. Cut against 3.1.1 (this flake's nixpkgs); verified to apply and pass on 3.1.0, which Bravais builds by making this flake follow its stable nixpkgs. `0008`, `0013` and `0015` have a 3.1.0 cut, picked by version, because 3.1.0's `real_main` differs on the lines they replace. MIT for jaq's text, Pathfinder's changes GPL-3.0-or-later OR MIT; never sent upstream. |
| `crates/pathfinder-core`, `-std`, `-json`, `-fmts` | jaq 3.1.1's engine crates, imported with the patch set applied (`crates/UPSTREAM.md`). Library names stay `jaq_core`, … so the sources stay diffable against upstream. |
| `crates/pathfinder-all` | jaq 3.1.1's `jaq-all` (lib `jaq_all`): compiling, running, reading and writing on the engine crates. |
| `crates/pathfinder-jaq` | jaq 3.1.1's CLI, with the CLI hunks of the patch set, as the library `jaq` the native engine runs (`jaq::run`) and as the binary `pathfinder-jaq`, run as `PATHFINDER_JAQ` to check the crates against the packaged jaq. `publish = false`; never installed. |
| `crates/pathfinder/tests/differential.rs` | The compatibility contract: byte-exact agreement with real jq on chosen cases. |
| `crates/pathfinder/tests/conformance.rs` | The coverage measure: jq's own suite, judged as jq's runner judges it. |
| `crates/pathfinder/tests/native.rs` | The native surface: mode cascade, JSON shapes, exit codes, and that the `jq` name is unchanged under an agent environment. |
| `fuzz/` | `cargo fuzz` targets for jq's argv, the front end, and evaluation through the engine crates; see Fuzzing. Its own Cargo root. |
| `SKILL.md` | Agent-facing description of the shim. Frontmatter first, so its licence is in `REUSE.toml`; keep `description` ≤ 1000 rendered characters. |

## Forbidden patterns

- `unsafe` — every crate sets `unsafe_code = "forbid"` through the workspace
  lint set.
- Forwarding an unrecognised flag to jaq.
- Polyfilling a name jq 1.8.1 does not define.
- Adding a third-party runtime dependency to the shim crate; it depends on the
  engine crates and std only. A dependency the engine needs goes into an
  engine crate, with `cargo audit` and a justification in
  `crates/UPSTREAM.md` (PRD R-015).
- A golden value in a test that was not produced by running real jq.

## Gotchas

- `prelude::render` returning `""` is the *good* case: it means the fast path is
  available.
- Tests that assert an engine-specific report set `PATHFINDER_ENGINE`
  themselves: the Nix package compiles in `jaq` as the default, a plain
  `cargo build` `native`.
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

Every file is covered by REUSE (Standard §4.3): Pathfinder's own files carry
the two SPDX tags. Software is `GPL-3.0-or-later`; docs are `CC-BY-SA-4.0`;
the embedded jq help text keeps upstream jq's MIT (§4.2). The engine crates
imported from jaq (`crates/pathfinder-{core,std,json,fmts,all,jaq}`) and the
patches in `packaging/jaq/` keep jaq's MIT for the text taken from jaq;
Pathfinder's changes to them, and the files Pathfinder adds among them, are
`GPL-3.0-or-later OR MIT`, so the expression for each such file is
`MIT AND (GPL-3.0-or-later OR MIT)` and the file as a whole stays usable
under MIT. They carry **no inline headers, by design**: `REUSE.toml` covers
them, so each file stays byte-diffable against upstream. Do not add headers
to them. The shim crate, `crates/pathfinder`, is `GPL-3.0-or-later` only. Commits to a Spacecraft Software remote must be signed
and show "Verified" (§6.3).
