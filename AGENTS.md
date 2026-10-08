# AGENTS.md — Pathfinder

Project-specific invariants for AI coding agents. General Rust and CLI rules come
from the Spacecraft Software skills (listed in [`CLAUDE.md`](CLAUDE.md)); this
file is only what is true *here*.

## What this is

A single-crate binary, `pathfinder`, installed with a `jq` symlink beside it. It
translates jq's command line into jaq's, rewrites the filter where jaq would run
it differently, and hands off. It parses jq (`src/syntax/`) but evaluates
nothing.

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
- **Measure the cost of a jq-level definition before adding one per element.**
  A filter-parameter call, `first`, and `if type == …` each cost hundreds of
  milliseconds per 100k calls under jaq; inline text and `label`/`break` were
  the measured winners (`tonumber`, key deletion).

## Conformance

`tests/conformance.rs` runs jq 1.8.1's own test suite (`tests/jq-suite/`,
vendored, MIT) through the shim — the coverage measure, as opposed to the
differential suite's statement of intent. `make conformance` prints per-file
pass rates; `V=1` lists every failure.

- `FLOOR` is a ratchet. Raise it in the change that raises the score; never
  lower it to make a change pass.
- `EXCLUDED` is generated from real jq 1.8.1, never hand-edited.
- A rising total can hide regressions: compare the failing-case lists before and
  after (`V=1`), not just the count. The first `match`/`test`/`capture` repair
  raised the total while breaking nine cases, two of them real call sites.

## Layout

| File | Owns |
|---|---|
| `src/jqargs.rs` | jq's argv grammar, transcribed from jq's `src/main.c`. Hand-rolled; clap cannot express it. |
| `src/scan.rs` | The jq token scanner — the crate's only real algorithm. Strings, `\(…)` interpolation nesting, comments, module headers. |
| `src/prelude.rs` | Polyfill and repair definitions, and the evidence for each. |
| `src/program.rs` | Program assembly: module header, prelude, `$ARGS` wrap, `$__loc__`. |
| `src/translate.rs` | jq argv → jaq argv. The only module that knows both dialects. |
| `src/post.rs` | `-a` and `--seq` byte transforms. |
| `src/native.rs` | `--explain` / `--install-shim`, reachable only under the native name. |
| `src/syntax/lex.rs`, `parse.rs` | jq's lexer and grammar, transcribed. Every node keeps its span. |
| `src/syntax/rewrite.rs` | Source-to-source rewrites: assignment, `del`, `?//`, `{$b: p}`, computed-key checks, compound `reduce` sources. |
| `src/syntax/check.rs` | jq's compile-time rejections, and jq's constant folding to decide them. |
| `src/syntax/print.rs` | Fully parenthesised printer: parser validation, and pattern text for rewrites. |
| `tests/differential.rs` | The compatibility contract: byte-exact agreement with real jq on chosen cases. |
| `tests/conformance.rs` | The coverage measure: jq's own suite, judged as jq's runner judges it. |

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
