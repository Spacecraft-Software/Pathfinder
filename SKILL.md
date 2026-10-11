---
name: pathfinder
description: >
  Pathfinder is a jq 1.8.1-compatible jq on a fork of jaq's engine, often
  installed as `jq`. Use this skill when a `jq` call behaves unexpectedly,
  when you need to know whether `jq` on PATH is real jq or Pathfinder, or
  when debugging how a jq filter is rewritten and which engine runs it.
  Covers `pathfinder --version` (identifies Pathfinder and its engine;
  `jq --version` deliberately says jq-1.8.1), `pathfinder --explain --json`
  (the rewritten program, the engine, the jaq argv), the PATHFINDER_ENGINE,
  PATHFINDER_JAQ and PATHFINDER_NO_REWRITE environment variables, exit codes,
  and where known divergences from jq are documented.
license: GPL-3.0-or-later
---

# Pathfinder

Pathfinder accepts jq 1.8.1's command line, rewrites the filter where its
engine would run it differently from jq, and evaluates it: in process on its
own engine (a fork of jaq's, `native`), or by handing off to a jaq binary
(`jaq`). Under the name `jq` it behaves as jq 1.8.1, byte for byte where it
can; its own verbs exist only under the name `pathfinder`.

## Is this `jq` Pathfinder?

`jq --version` prints `jq-1.8.1` either way, on purpose: scripts grep it.
Ask the native name instead:

```sh
pathfinder --version
```

It prints Pathfinder's version and the engine it runs: `native` (in
process), or `jaq` with the binary and where that came from
(`PATHFINDER_JAQ`, `pinned` or `PATH`). To check that `jq` itself is the shim:

```sh
readlink -f "$(command -v jq)"
```

## Debug a filter

Put `pathfinder --explain --json` in front of the jq arguments. Nothing runs;
one JSON document on stdout shows what the engine would receive:

```sh
pathfinder --explain --json -c '.a.b = 1'
```

`data.engine` is `native` or `jaq`, `data.program.rewritten` says whether the
filter was changed (and `text` shows the result), `data.execution` is
`in-process`, `exec` or `pipe`, and `data.jaq_argv` is what a jaq binary
would receive. To see the engine's own behaviour without any rewriting:

```sh
printf '{}' | PATHFINDER_NO_REWRITE=1 jq -c '.a.b = 1'
```

## Output mode of the native verbs

`--explain`, `--version` and `--install-shim` print one JSON document
(`{"metadata":…,"data":…}`) when given `--json`, when `AI_AGENT` or `AGENT`
is set to anything non-empty, when `CI` is set and not `false`/`0`, or when
stdout is not a terminal; otherwise text. Any failure of those verbs in JSON
mode is one `{"error":{"code":…,"exit_code":…,"message":…}}` line on stderr,
with a runnable `hint` when one is known and nothing on stdout. Codes:
`INVALID_ARGUMENT` and `MISSING_ARGUMENT` (exit 2), `UNSUPPORTED` (2),
`COMPILE_ERROR` (3, jq's compile code), `GENERAL` (1), `PERMISSION_DENIED` (4),
`CONFLICT` (5, `--install-shim` without `--force`). `--json` is not a jq flag: without
one of those verbs, `pathfinder --json .` and `jq --json .` exit 2.

## Environment

| Variable | Effect |
|---|---|
| `PATHFINDER_ENGINE` | `native` (in process) or `jaq` (a jaq binary); unset, the build's default. Anything else exits 2. |
| `PATHFINDER_JAQ` | Path to the jaq binary for the `jaq` engine; wins over the build-time pin and `PATH`. |
| `PATHFINDER_NO_REWRITE` | Set to anything: hand the filter to jaq unparsed and unrewritten. |

## Exit codes

Under `jq`, jq's: `0` success, `1` with `-e` and a false or null last
output, `2` usage error, `3` compile error, `5` runtime or input error. `127`
means the jaq engine's binary could not be executed: set `PATHFINDER_JAQ`, or
`PATHFINDER_ENGINE=native`.

`pathfinder --install-shim <dir>`: `0` installed, `5` a different `jq`
already exists (re-run with `--force`), `4` permission denied, `1` any other
failure.

## Cautions

- jq has no `-i`/`--in-place`. jaq does, but Pathfinder rejects it like jq
  (exit 2); never rely on a `jq` command editing a file.
- Not every jq behaviour is reproduced. Known, measured differences are in
  [`doc/DIVERGENCES.md`](doc/DIVERGENCES.md); check there before assuming a
  difference is a bug in your filter.
