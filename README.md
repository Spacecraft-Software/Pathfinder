<!--
SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
SPDX-License-Identifier: CC-BY-SA-4.0
-->

# Pathfinder

**A jq 1.8.1-compatible `jq` in safe Rust, on an engine grown from [`jaq`](https://github.com/01mf02/jaq)'s.**

Pathfinder accepts jq's exact command line and runs jq programs as jq 1.8.1
does: its numbers, its error messages, its regular expressions (a port of
the Oniguruma jq ships) and its date functions (glibc's, as jq's release
binary runs them). Install it as `jq` and existing scripts keep working.

## Why

`jaq` is faster than jq and written in safe Rust, which is why it is the
Spacecraft Software default. It is also *not* a drop-in, despite being widely
described as one. Measured against jq 1.8.1:

- Nine jq command-line flags make jaq exit 2 with `unknown flag`.
- Twenty-two jq builtins are `undefined filter`.
- **jaq does not auto-vivify**: `null | .a = 1` and `{} | .a.b = 1` error, where
  jq builds the containers. This is the one that breaks real scripts, and it is
  not mentioned in any of the "drop-in replacement" write-ups.

`alias jq = jaq` hides all of that until it fails at runtime. Pathfinder began
as a shim in front of jaq that closed those gaps by rewriting filters; since
0.2.0 it is its own jq. The engine is a hard fork of jaq's, in this
repository, changed until it behaves as jq does, and it runs in process: one
`execve`, nothing spawned. The few places it still differs are measured and
listed.

## Status

| | |
|---|---|
| Baseline | jq **1.8.1** |
| Engine | Pathfinder's engine crates (forked from jaq 3.1.1), in process |
| Differential suite | **1 174 invocations**, byte-identical stdout and exit code against jq 1.8.1 |
| jq's own test suite | **100%** (820/820) — `make conformance` |
| Known unrepaired divergences | All in [`doc/DIVERGENCES.md`](doc/DIVERGENCES.md), measured |

## Install

With Nix, from this flake:

```sh
nix run github:Spacecraft-Software/Pathfinder -- -r '.a' in.json   # try it
```

To daily-drive it, add the flake as an input to your system or Home Manager
configuration and put `packages.${system}.pathfinder-jq` in `home.packages`.
That variant ships `bin/jq -> pathfinder`; `packages.${system}.default` is the
binary alone, for when you want Pathfinder without it answering to `jq`. Both
also ship `bin/mjq`, a short name for `pathfinder` itself. Taking
over the `jq` name is always an explicit choice.

The package is one binary with no runtime dependencies and no wrapper script:
a `jq` call is exactly one `execve`.

Without Nix:

```sh
cargo build --release
./target/release/pathfinder --install-shim ~/.local/bin
```

That creates `~/.local/bin/jq` pointing at the binary, and refuses to overwrite
an existing `jq` without `--force`; `make install` also installs `mjq`. Check that `~/.local/bin` comes before any real jq on your
`PATH`, or the symlink never wins.

Pathfinder never edits your shell configuration.

## Use

Exactly as jq:

```sh
jq -r '.items[] | select(.active) | .name' data.json
jq -s 'add' a.json b.json
jq -n --jsonargs '$ARGS.positional' 1 '{"k":2}'
```

Under its own name it also answers questions of its own:

```sh
pathfinder --explain -c '.[1,2] |= empty'          # what would the engine run?
pathfinder --version                               # which Pathfinder?
pathfinder --install-shim ~/.local/bin [--force]
```

`--explain` prints the engine, the assembled program (and what was rewritten
or injected into it), and how input and output are framed. `--version` names
Pathfinder's version, the jq it reproduces, its engine, the maintainer and
the website. `--install-shim` exits 5 when a different `jq` is already
there and `--force` was not given, and 4 when the directory is not writable.

These verbs are unavailable through the `jq` symlink on purpose: a script
passing `--explain` to what it believes is jq must get jq's `Unknown option`
error, and `jq --version` prints `jq-1.8.1`, because scripts grep for it.

Each verb reports as text or as one compact JSON document,
`{"metadata":{…},"data":{…}}`, following the Spacecraft CLI Standard's
cascade — first match wins:

1. `--json` beside the verb;
2. `AI_AGENT` or `AGENT` set to anything non-empty;
3. `CI` set, and not `false` or `0`;
4. stdout is a terminal: text;
5. otherwise: JSON.

```sh
pathfinder --explain --json -c '.a.b = 1'
```

Every failure of a verb in JSON mode is written to stderr as one line,
`{"error":{"code":…,"exit_code":…,"message":…,"hint":…,…}}`, with nothing on
stdout. The hint is a command that can be run as is, and is omitted where no
such command is known:

| `code` | Exit | When |
|---|---|---|
| `INVALID_ARGUMENT` | 2 | jq's grammar rejects the command line |
| `MISSING_ARGUMENT` | 2 | `--explain` with no filter on a terminal, or `-f` with no file |
| `UNSUPPORTED` | 2 | a jq feature Pathfinder cannot provide (`--stream`, `modulemeta`) |
| `COMPILE_ERROR` | 3 | a filter jq refuses to compile; 3 is jq's own code, kept so both names agree, although the CLI Standard's table gives 3 to `NOT_FOUND` |
| `GENERAL` | 1 | `--install-shim` could not create the link |
| `PERMISSION_DENIED` | 4 | `--install-shim` into a directory that is not writable |
| `CONFLICT` | 5 | `--install-shim` found a `jq` that is not Pathfinder's |

In text mode the same failures print jq's own wording. None of this reaches
the `jq` name: there, `--json` is an unknown option, and an agent environment
changes nothing. Agents should read [`SKILL.md`](SKILL.md).

## How it works

```
argv[0] ──► parse jq's command line ──► check the program ──► run the engine
              (jq's grammar)              (jq's parser,          (in process)
                                           jq's errors)
```

- **jq's grammar, jq's errors.** The command line is parsed by a transcription
  of jq's `main.c`; the program by a port of jq's lexer and bison grammar, so
  a syntax error is reported where and as jq reports it.
- **The program is left byte-identical** unless it needs something. Two
  things are rewritten: an update of several paths (`(.a, .b) |= f`), whose
  deletions jq defers to the end, and an unknown `@format`, which jq rejects
  at run time. A `def` of your own wins over anything injected.
- **Unknown flags are rejected, never forwarded.** The engine's driver
  started as jaq's command line, which has `-i/--in-place`; nothing of the
  user's argv reaches it except through an option set from a jq flag.

## Scope

Pathfinder will not silently change what a filter means. Where it still
differs from jq — `--stream`, module introspection, a date past the year
5 881 600 — it refuses or documents rather than guessing.

It also refuses to be *more* permissive than jq. `leaf_paths`, `ascii`,
`isvalid`, `toarray`, `ANY`, `ALL`, `@base32` and friends are not jq 1.8.1
builtins, so Pathfinder does not define them.
Accepting a filter that real jq rejects would let a script pass here and break on
the next machine.

## Testing

```sh
make check      # fmt + clippy + tests
cargo test      # unit tests, the differential suite and jq's own suite
```

The differential suite runs every case through **real jq** and through the shim
and compares bytes and exit codes. Real jq does not have to be installed: it is
located via `PATHFINDER_REAL_JQ`, or fetched ephemerally with
`nix build nixpkgs#jq`, and the suite skips if neither is available. Set
`PATHFINDER_REQUIRE_JQ=1`, as CI does, to make that skip a hard failure.

The jq it finds must be **1.8.1**, the declared baseline, and the suite checks
rather than assumes. jq's own surface moves between releases — `trimstr` does
not exist before 1.8, and `builtins` answers 218 in 1.7.1 against 226 in 1.8.1 —
so an older jq produces confident-looking mismatches that are really just
version drift.

This matters more than it sounds. Hand-written golden values encode what the
author believed jq does — and while building this, four such beliefs turned out
to be wrong.

## Project Posture

Personal / hobby project, developed at hobby pace around the maintainer's own
use. No warranty, no SLA, no support commitment — see [`NOTICE.md`](NOTICE.md).
Contributions and suggestions are welcome and accepted at the maintainer's
discretion; see [`CONTRIBUTING.md`](CONTRIBUTING.md). Forking under
GPL-3.0-or-later is always available and encouraged when goals diverge.

Prior work this is built on is credited in [`CREDITS.md`](CREDITS.md).

---

Maintainer: Mohamed Hammad &lt;Mohamed.Hammad@SpacecraftSoftware.org&gt;
License: GPL-3.0-or-later ·
Website: <https://Pathfinder.SpacecraftSoftware.org/>

*--- Forged in Spacecraft Software ---*
