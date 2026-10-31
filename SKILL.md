---
name: pathfinder
description: >
  Pathfinder is a jq 1.8.1-compatible jq in safe Rust, with an engine of
  its own, often installed as `jq`. Use this skill when a `jq` call behaves
  unexpectedly, when you need to know whether `jq` on PATH is real jq or
  Pathfinder, or when debugging how a jq filter is assembled before it runs.
  Covers `pathfinder --version` (identifies Pathfinder; `jq --version`
  deliberately says jq-1.8.1), `pathfinder --explain --json` (the program
  as the engine runs it, and how input and output are framed), the
  PATHFINDER_ENGINE and PATHFINDER_NO_REWRITE environment variables, exit
  codes, and where known divergences from jq are documented.
license: GPL-3.0-or-later
---

# Pathfinder

Pathfinder accepts jq 1.8.1's command line and runs the program in process
on its own engine (grown from jaq's). Under the name `jq` it behaves as
jq 1.8.1, byte for byte where it can; its own verbs exist only under the
name `pathfinder`.

## Is this `jq` Pathfinder?

`jq --version` prints `jq-1.8.1` either way, on purpose: scripts grep it.
Ask the native name instead:

```sh
pathfinder --version
```

It prints Pathfinder's version, the jq it reproduces and its engine
(`native`, in process). To check that `jq` itself is Pathfinder:

```sh
readlink -f "$(command -v jq)"
```

## Debug a filter

Put `pathfinder --explain --json` in front of the jq arguments. Nothing runs;
one JSON document on stdout shows what the engine would receive:

```sh
pathfinder --explain --json -c '.[1,2] |= empty'
```

`data.program.rewritten` says whether the filter was changed (and `text`
shows the result); `data.stdout` and `data.stdin` say how output and input
are framed (`-a`, `--seq`, several files read as one stream). To see the
engine's own behaviour without any rewriting:

```sh
printf '[1,2,3,4]' | PATHFINDER_NO_REWRITE=1 jq -c '.[1,2] |= empty'
```

That prints `[1,3]`, the engine deleting one index at a time; with the
rewrite (and in jq) it is `[1,4]`.

## Discover the surface

`pathfinder --describe --json` lists every command, the environment variables
and the conventions in one document. `pathfinder --schema` prints each
command's JSON Schema (Draft 2020-12); `pathfinder --schema --format
anthropic` (or `openai`, `gemini`, `mcp`) gives them as that provider's tool
definitions, and `pathfinder --schema pathfinder_explain` just one.

## Output mode of the native verbs

`--explain`, `--version`, `--install-shim` and `--describe` print one JSON document
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
| `PATHFINDER_ENGINE` | Unset, empty or `native`. Anything else exits 2: the `jaq` engine was removed in 0.2.0. |
| `PATHFINDER_NO_REWRITE` | Set to anything: run the filter without Pathfinder's rewrites. |

## Exit codes

Under `jq`, jq's: `0` success, `1` with `-e` and a false or null last
output, `2` usage error, `3` compile error, `5` runtime or input error.

`pathfinder --install-shim <dir>`: `0` installed, `5` a different `jq`
already exists (re-run with `--force`), `4` permission denied, `1` any other
failure.

## Cautions

- jq has no `-i`/`--in-place`, and Pathfinder rejects it like jq (exit 2);
  never rely on a `jq` command editing a file.
- Not every jq behaviour is reproduced. Known, measured differences are in
  [`doc/DIVERGENCES.md`](doc/DIVERGENCES.md); check there before assuming a
  difference is a bug in your filter.
