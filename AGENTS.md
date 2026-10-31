# AGENTS.md — Pathfinder

Project-specific invariants for AI coding agents. General Rust and CLI rules come
from the Spacecraft Software skills (listed in [`CLAUDE.md`](CLAUDE.md)); this
file is only what is true *here*.

## What this is

A binary, `pathfinder`, installed with a `jq` symlink beside it. It parses
jq's command line and filter (`src/jqargs.rs`, `src/syntax/`), rewrites the
little the engine still runs differently from jq, and evaluates it on
Pathfinder's engine crates (a hard fork of jaq 3.1.1, grown to jq's
semantics), linked in and run in process (`src/engine.rs`). Nothing is
spawned.

It answers to three names: `pathfinder`, `mjq` (a symlink the packages
install, the same surface as `pathfinder`) and `jq` (jq's surface only;
`compat::detect`).

Until 0.2.0 a jaq binary could stand in for the engine
(`PATHFINDER_ENGINE=jaq`); that engine, its patch set and its rewrites are
gone (PLAN P-078). `PATHFINDER_ENGINE` is still read: unset or `native`
runs, anything else exits 2, so a script that asks for jaq fails loudly.

The repository is a Cargo workspace. The shim is `crates/pathfinder`; paths
like `src/…` and `tests/…` in this file are relative to it. The root
`Cargo.toml` holds the lint set and the release profile, and its
`default-members` keeps a bare `cargo build`/`test`/`clippy` on the shim;
`--workspace` reaches every crate.

Baseline is **jq 1.8.1**; jq's own suite passes 820/820 (`FLOOR`).
`input_filename` and `input_line_number` are answered from the driver's
reader (`jq::Position`): `null` before any input is read, then the file the
reader has open, across several files too.

## Build, test, lint

```sh
cargo build --release
cargo test                       # the shim: unit, differential, conformance, native
cargo test --workspace --exclude pathfinder-jq   # the engine crates
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
make check                       # all of the above
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
printf '{}' | target/release/pathfinder -c '.a.b = 1'
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
- No third-party dependency for this: `src/json.rs` writes the JSON.
  `serde_json` is a dev-dependency, used by `tests/native.rs` to parse the
  output.
- `--version` and `--explain` name the engine (`native`); a
  `PATHFINDER_ENGINE` naming another fails under either name with exit 2, as
  a structured error in JSON mode.

## Architectural invariants

- **Whitelist, never pass through.** Every option the engine's driver
  receives is set in `translate.rs` from a jq flag it named. This is a
  *safety* property, not tidiness: the driver is jaq's command line as a
  library, and jaq has `-i/--in-place` where jq does not, so nothing of the
  user's argv reaches it except through a field set there.
- **The engine is jaq's CLI as a library** (`crates/pathfinder-jaq`, lib
  `jaq`): `jaq::run` takes the run options (`jaq::Cli`, built by
  `translate::plan`), stdin, the concatenated files' `Segment`s and stdout,
  and returns the exit status. It never exits the process itself (`halt`
  included), so the caller flushes first. Several files are one
  `inputs::reader` stream described by `inputs::segments`, `--seq` input
  goes through `post::StripRs`, and `-a`/`--seq` output through
  `post::Framer`.
- **Do not become more permissive than jq.** A name is defined only if jq
  1.8.1 defines it. Accepting a filter real jq rejects lets a script pass here
  and fail on the next machine — the worst outcome for a shim.
- **The filter stays byte-identical unless something is needed.** `assemble`
  returns `rewritten: false` in that case and `-f` is kept, preserving the
  error provenance of the file.
- **Prelude on one line.** Errors are reported against the program text; a
  prelude on its own line would shift every user line number by one.
- **stderr is the engine's own**: diagnostics, `debug` output and their
  interleaving with stdout are written by the driver as jq writes them.

### The parser and the rewriter (`src/syntax/`)

- **The engine compiles what Pathfinder's parser reads** (M21,
  `src/front.rs`): `jaq::Cli::front_end` hands jaq's loader
  `front::main_module`, which turns the assembled program's syntax tree into
  the engine's terms. Every string in them is a slice of the program text,
  because the engine finds a name's position from where its slice points:
  a use is the slice its node spans, a binding name any slice spelling it.
  A module header, and what jq rejects (a syntax error, `\v`, a lone high
  surrogate), answer `None` and jaq's parser reads them, as before M21.
  jq's `?` after a postfix step makes that step optional (`1 | .a.b?` still
  fails at `.a`); after anything else, a second `?` included, it is `try`.
  `PATHFINDER_FRONT_END=jaq` puts jaq's parser back, to compare the two.
  Whenever jaq's parser reads a program (that switch, or an
  `import`/`include` header), the program takes the syntax rewrites
  (`Rewrites::syntax`), which write jq's syntax as jaq's parser reads it.

- **A syntax error is found by jq's own grammar** (M21, P-069,
  `syntax/jq_errors.rs`): bison 3.8's driver over the tables bison generated
  for jq 1.8.1 (`syntax/jq_tables.rs`, written by `tools/jq-tables.py` from
  jq's release tarball, then `rustfmt`; never edited), fed by a port of
  `lexer.l`. It runs on every program first, so a program is a syntax error
  exactly when jq's grammar says so, worded and located as jq does
  (`unexpected X, expecting …`, the error rules' `Possibly unterminated
  'if' statement`, the lexer's `Invalid escape …`), and an accepted program
  with no main expression is `Top-level program not given`. Every lexer
  match, whitespace and comments included, moves the location, as jq's
  `YY_USER_ACTION` does. The semantic errors jq raises inside a parse that
  also has a syntax error (a constant non-string key, module metadata) are
  not reported with it; module loading errors are jaq's (T-035).
- **Undefined names are reported in jq's compile order** (`check::Levels`):
  a function body's own names in its instruction order (an index's key
  before its base, a `reduce`'s init before its source, a C builtin's
  arguments inline, `jq_tables::C_BUILTINS`), then its closures (another
  call's arguments, both sides of `=`/`|=`, the target of `op=`) and
  definitions, in order, only if the body had no error itself.
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
  `builtins` (`prelude::JQ_BUILTINS`), one of its internal `_` builtins
  (`check::JQ_INTERNAL`, measured), a literal, nor defined by the program
  (`check::undefined_calls`). That covers jaq's own helpers
  (`isarray`, `tobytes`, `halt/1`, …), which a filter could otherwise call,
  and replaces jaq's report of an unknown name, which quoted the whole
  prelude. It follows jq's scoping: a definition counts only once something
  reachable calls it, arguments always count, and an `import`/`include` or a
  `~/.jq` file turns it off. A call to such a name in code nothing reaches is
  replaced by `empty` (`check::unreachable_undefined`): jq never compiles
  it, the engine compiles every definition. The engine's own helpers are
  `_`-prefixed, so a filter cannot call them (P-066); an unknown `@format`
  is rewritten to jq's run-time error (`JQ_FORMATS`). Variables and labels
  are checked by the same walk, in jq's scopes (`$x is not defined`,
  `$*label-x is not defined`): `$ENV`, `$ARGS` and the command line's named
  arguments (`prelude::Context::named`) are always bound; a `reduce`/`foreach`
  init and a pattern's computed keys see only the outer scope; a definition
  sees the variables where it is written.
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
- **Assignment is native** (M20): the engine creates
  containers and raises jq's errors, and only a comma target with an update
  (`(.a, .b) |= f`) is rewritten, to `_pf_modify`, whose deletions jq defers
  to the end (`Rewrites::comma_update`). Keep the engine's update from
  holding a second reference to the value it updates: `Rc::make_mut` then
  copies it on every step, which made a 100 000-key `reduce` take 10^12
  instructions; `setpath` is the native `_setpath` for the same reason
  (`fromstream` went quadratic through `getpath($p) = $x`). An update to
  nothing deletes the path, as jq's does: a step whose child is absent
  (`ValT::jq_absent`) and still `null` after the rest of the path yields
  nothing, so the parent creates no key and pads no array.
- **Arguments run in jq's order.** jq runs a C builtin's last argument in
  the outermost loop and a definition's first (`def f($a; $b)` is
  `a as $a | b as $b | …`). The engine's natives bind as C builtins do
  (`filter::bind_native`), so a native standing for a jq *definition* sits
  behind one that binds its `$` parameters (`range/3` → `_range`), and a
  definition standing for a C builtin binds them last first (`setpath`).
- **`del` and `delpaths` are jq's** (M20): the engine's `delpaths` is a
  port of jq's `jv_delpaths`.
- **Regexes are jq 1.8.1's Oniguruma** (M22,
  `crates/pathfinder-regex`): Oniguruma 6.9.10, the one jq's release
  binary vendors, in its one configuration (`ONIG_SYNTAX_PERL_NG`, UTF-8,
  `ONIG_OPTION_CAPTURE_GROUP`). The parser transcribes `regparse.c` (a
  pattern is accepted, rejected and worded as jq does, bugs included), the
  tune pass the parts of `regcomp.c` that change a result (case folds
  unravelled into strings and classes, lookbehinds checked and measured
  after `alt_reduce` and `tune_next`, backreferences checked first), the
  compiler keeps Oniguruma's instruction shapes (they decide where an
  empty iteration ends a loop and what an atomic group cuts), and the
  matcher keeps its stack discipline and works on byte positions, as
  Oniguruma does: jq resumes one byte past an empty match, so a search
  can start inside a character. `Perl_NG`'s `^`/`$` are `\A`/`\Z` (its
  default option is `SINGLELINE`), `m` lets `.` match a newline, `l` is
  the longest match of every start, `n` rejects empty matches. Below 256
  a character type comes from Oniguruma's ISO-8859-1 table, not the
  ranges a class is built from (`\w` matches `½`, `[\w]` does not).
  `src/tables.rs` is generated by `tools/onig-tables.py` from jq's
  tarball and never edited. `pathfinder-std`'s `_match_impl` is jq's
  `f_match` (offsets counted with jq's own loop, continued rather than
  restarted), and `defs.jq` holds jq's definitions of `match`, `test`,
  `capture`, `scan`, `splits`, `split/2`, `sub` and `gsub` verbatim; the
  engine's string slice remembers one string's character starts, so their
  slicing once per match stays linear. Measure a regex claim with
  `tools/regex-diff.py`, which found the quirks above: random patterns
  over every construct and flag, run through jq 1.8.1 and the shim,
  compared line by line. jq's own limit applies: its `main` lowers
  Oniguruma's parse depth limit to 1024, so 512 nested groups are
  `parse depth limit over`.
- **The time builtins are glibc's** (M23,
  `pathfinder-std/src/ctime.rs`): jq's `gmtime`, `mktime`, `strftime`,
  `strptime` and their local forms call the C library, and its release
  binary links glibc 2.39, so `ctime.rs` transcribes that release's
  `__strftime_internal`, `__strptime_internal` and `__mktime_internal` in
  the C locale, their order of checks and their quirks kept (`%+` is not a
  flag, `#` is ignored in a subformat, `-` still pads a name, an
  abbreviated day moves `%a`'s read position, a `%O` with a number decides
  the locale). `time.rs` is jq's C glue over it; `todate`/`fromdate` are
  jq's definitions. Only the zone's rules come from `jiff`. Measure a
  time claim with `tools/time-diff.py` under several `TZ`; a change here
  is checked against glibc's source, not against a man page.
- **Measure the cost of a jq-level definition before adding one per element.**
  A filter-parameter call, `first`, `type` (about 4 µs under jaq 3.1's engine) and
  `and`/`or` (several µs each) are expensive; comparisons (`. < ""` is "not a
  string, number or later type"), nested `if`s, inline text and `label`/`break`
  are cheap. Type tests in a per-element path are written as comparisons.
- **Errors are worded as jq words them** (M20): the core's
  `Error::typ`/`math`/`index` and the builtins raise jq 1.8.1's messages,
  built with `ValT::jq_quoted`. An uncaught error is reported by the driver
  (`pathfinder-jaq/src/jq.rs`, `wording`) with jq's location;
  `errors_are_reported_as_jq_reports_them` compares stderr and exit status
  with real jq.
- **The error location counts lines the way jq reads them**: `fgets` into a
  4096-byte buffer, so up to a newline or 4095 bytes at a time, and through
  the chunk holding the last byte the parser needed (the delimiter after a
  number or literal, the closing character otherwise). On
  stdin `jq::Lines` is the only buffer and serves single bytes without a call;
  a reader layered under std's `Read::bytes` cost up to 66% on one long line.
  Measure stdin with `perf stat -e cycles:u`, one run per invocation (`-r`
  gives later runs an empty stdin).
- **A parse error is worded by jq's own parser, transcribed.**
  `pathfinder-json/src/parse_error.rs` is `jv_parse.c`'s state
  machine without the values; jaq's parser builds the values, and on failure
  the value that failed is re-scanned from jq's initial state. On stdin
  `jq::Position` implements `Recall`, keeping that value's text only when the
  line buffer is compacted. jaq's reader rejects what jq's rejects (comments,
  byte strings, non-string keys, run-on literals) so that both stop at the
  same byte; loosening it makes the two disagree on where an error is.
- **A path expression fails where jq's does**: a non-path filter in path
  mode yields a *lost* path (`Node::Lost`), which fails only when a later
  step uses it. The messages start `Invalid`, which `jq.rs::wording` passes
  through unchanged; keep it so.

## Conformance

`tests/conformance.rs` runs jq 1.8.1's own test suite (`tests/jq-suite/`,
vendored, MIT) through the shim — the coverage measure, as opposed to the
differential suite's statement of intent. `make conformance` prints per-file
pass rates; `V=1` lists every failure.

- `FLOOR` is a ratchet, now at 820 of 820. Raise it in the change that
  raises the score; never lower it to make a change pass.
- `EXCLUDED` is generated from real jq 1.8.1, never hand-edited.
- A rising total can hide regressions: compare the failing-case lists before and
  after (`V=1`), not just the count. The first `match`/`test`/`capture` repair
  raised the total while breaking nine cases, two of them real call sites.

## Fuzzing

`fuzz/` is a `cargo fuzz` crate with its own Cargo root, outside the
workspace. It has no lib target to link against, so `fuzz/src/lib.rs`
includes the shim's modules by `#[path]` (`jqargs`, `scan`, `prelude`,
`program`, `syntax`); a new `crate::` dependency of those needs a
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
  timeout or OOM there is a finding only if real jq is faster on it: run
  the artifact through both. M24's 24 h run found `range("")` counting for
  ever that way, and `syntax` a chain of `?//` growing threefold per link.
  The target's first finding, a panic on `[1 #`, is fixed in the engine's
  lexer.
- A crash is reproduced with `cargo fuzz run -s none <target> <artifact>`.
  A crash gets a fix and a test where it happened: a unit test in the shim,
  a differential case or an engine crate test in the engine.
- A job can hang after its timeout fires (`-s none`, M24: `ALARM: working
  on the last Unit`, then no CPU for five hours), and `-fork`'s parent waits
  on it. In a long run, check that each target's log still grows; killing
  the stuck job lets the parent go on.
- `fuzz/seeds/` is committed (jq's suite for `syntax`/`engine`); `corpus/`,
  `artifacts/` and `target/` are not.

## Layout

| File | Owns |
|---|---|
| `Cargo.toml` | The workspace: members, `default-members`, the shared lint set (`[workspace.lints]`) and the release profile. One `Cargo.lock`, at the root. |
| `crates/pathfinder/src/jqargs.rs` | jq's argv grammar, transcribed from jq's `src/main.c`. Hand-rolled; clap cannot express it. |
| `crates/pathfinder/src/scan.rs` | The jq token scanner — the crate's only real algorithm. Strings, `\(…)` interpolation nesting, comments, module headers. |
| `crates/pathfinder/src/prelude.rs` | The two definitions left to inject: jq's `builtins` list and its `_modify`. |
| `crates/pathfinder/src/program.rs` | Program assembly: module header, prelude, `$ARGS` wrap, `$__loc__`. |
| `crates/pathfinder/src/translate.rs` | jq's command line → the engine's run options (`jaq::Cli`); whitelist only. |
| `crates/pathfinder/src/front.rs` | The front end: the syntax tree as the engine's terms, slices of the program text. |
| `crates/pathfinder/src/engine.rs` | Running a plan on the engine in process; refusing a `PATHFINDER_ENGINE` other than `native`. |
| `crates/pathfinder/src/post.rs` | `-a` and `--seq` byte transforms: `Framer` (output) and `StripRs` (input). |
| `crates/pathfinder/src/inputs.rs` | Several input files: opened before the engine starts, read as one stream, and described to it as `Segment`s. |
| `crates/pathfinder/src/native.rs` | `--explain` / `--version` / `--install-shim`, their text and JSON reports, and structured errors; reachable only under the native name. |
| `crates/pathfinder/src/describe.rs` | `--describe` and `--schema`: the native surface described from one table of commands. |
| `crates/pathfinder/src/mode.rs` | The native verbs' output-mode cascade (CLI Standard §5). |
| `crates/pathfinder/src/json.rs` | Minimal JSON writer (RFC 8259 escaping) for the native reports; std-only. |
| `crates/pathfinder/src/syntax/lex.rs`, `parse.rs` | jq's lexer and grammar, transcribed. Every node keeps its span. |
| `crates/pathfinder/src/syntax/rewrite.rs` | Source-to-source rewrites: an update of several paths, `?//`, unknown formats, and jq's syntax for jaq's parser. |
| `crates/pathfinder/src/syntax/jq_errors.rs`, `jq_tables.rs` | jq's syntax errors: its lexer ported, and bison's driver over jq 1.8.1's generated tables (`tools/jq-tables.py`). |
| `crates/pathfinder/src/syntax/check.rs` | jq's compile-time rejections, and jq's constant folding to decide them. |
| `crates/pathfinder/src/syntax/print.rs` | Fully parenthesised printer: parser validation, and pattern text for rewrites. |
| `crates/pathfinder-core`, `-std`, `-json`, `-fmts` | jaq 3.1.1's engine crates, imported with Pathfinder's patch set applied and grown since (`crates/UPSTREAM.md`). Library names stay `jaq_core`, … so the sources stay diffable against upstream. |
| `crates/pathfinder-std/src/ctime.rs` | glibc 2.39's `strftime`, `strptime`, `mktime`/`timegm` and `gmtime`/`localtime` in Rust, as jq's release binary runs them (M23); `LGPL-2.1-or-later` for glibc's text. |
| `crates/pathfinder-regex` | jq 1.8.1's Oniguruma 6.9.10 in Rust (M22): `parse.rs` (`regparse.c`), `tune.rs` and `compile.rs` (`regcomp.c`), `exec.rs` (`regexec.c`), `error.rs` (`regerror.c`), `tables.rs` (its Unicode data, generated). No dependencies; Oniguruma's BSD-2-Clause and notice on what is taken from it. |
| `crates/pathfinder-all` | jaq 3.1.1's `jaq-all` (lib `jaq_all`): compiling, running, reading and writing on the engine crates. Its `defs.jq` holds definitions that need every engine crate (`format`). |
| `crates/pathfinder-jaq` | jaq 3.1.1's CLI, with the CLI hunks of the patch set, as the library `jaq` the shim runs (`jaq::run`). `publish = false`; never installed. |
| `crates/pathfinder/tests/differential.rs` | The compatibility contract: byte-exact agreement with real jq on chosen cases. |
| `crates/pathfinder/tests/conformance.rs` | The coverage measure: jq's own suite, judged as jq's runner judges it. |
| `crates/pathfinder/tests/native.rs` | The native surface: mode cascade, JSON shapes, exit codes, and that the `jq` name is unchanged under an agent environment. |
| `tools/jq-tables.py` | Writes `jq_tables.rs` from jq 1.8.1's `src/` (parser tables, C builtins). |
| `tools/regex-diff.py` | Random regexes through jq 1.8.1 and the engine, compared case by case (M22). |
| `tools/time-diff.py` | Random time calls through jq 1.8.1 and the engine under several zones, compared case by case (M23). |
| `tools/onig-tables.py` | Writes `pathfinder-regex`'s `tables.rs` from the Oniguruma in jq 1.8.1's tarball (properties, folds, grapheme breaks, ISO-8859-1 types). |
| `fuzz/` | `cargo fuzz` targets for jq's argv, the front end, and evaluation through the engine crates; see Fuzzing. Its own Cargo root. |
| `SKILL.md` | Agent-facing description of the shim. Frontmatter first, so its licence is in `REUSE.toml`; keep `description` ≤ 1000 rendered characters. |

## Forbidden patterns

- `unsafe` — every crate sets `unsafe_code = "forbid"` through the workspace
  lint set.
- Passing an unrecognised flag through to the engine's driver.
- Polyfilling a name jq 1.8.1 does not define.
- Adding a third-party runtime dependency to the shim crate; it depends on the
  engine crates and std only. A dependency the engine needs goes into an
  engine crate, with `cargo audit` and a justification in
  `crates/UPSTREAM.md` (PRD R-015).
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

Every file is covered by REUSE (Standard §4.3): Pathfinder's own files carry
the two SPDX tags. Software is `GPL-3.0-or-later`; docs are `CC-BY-SA-4.0`;
the embedded jq help text keeps upstream jq's MIT (§4.2). The engine crates
imported from jaq (`crates/pathfinder-{core,std,json,fmts,all,jaq}`) keep jaq's MIT for the text taken from jaq;
Pathfinder's changes to them, and the files Pathfinder adds among them, are
`GPL-3.0-or-later OR MIT`, so the expression for each such file is
`MIT AND (GPL-3.0-or-later OR MIT)` and the file as a whole stays usable
under MIT. They carry **no inline headers, by design**: `REUSE.toml` covers
them, so each file stays byte-diffable against upstream. Do not add headers
to them. The shim crate, `crates/pathfinder`, is `GPL-3.0-or-later` only.
`crates/pathfinder-std/src/ctime.rs` transcribes glibc and is
`LGPL-2.1-or-later AND MIT AND (GPL-3.0-or-later OR MIT)` in `REUSE.toml`.
`crates/pathfinder-regex` carries inline headers: its generated tables are
Oniguruma's `BSD-2-Clause` (K.Kosako's notice), the files transcribing
Oniguruma's code `BSD-2-Clause AND (GPL-3.0-or-later OR MIT)`, the rest
`GPL-3.0-or-later OR MIT`. Commits to a Spacecraft Software remote must be signed
and show "Verified" (§6.3).
