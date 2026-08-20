<!--
SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
SPDX-License-Identifier: CC-BY-SA-4.0
-->

# Pathfinder

**A `jq`-compatible shim over [`jaq`](https://github.com/01mf02/jaq).**

Pathfinder accepts jq's exact command line, translates it into jaq's, rewrites
the filter when a missing builtin needs supplying, and hands off to jaq. Install
it as `jq` and existing scripts keep working.

Not a jq implementation: it implements none of the jq language. jaq does the
work; Pathfinder is the translation layer in front of it.

## Why

`jaq` is faster than jq and written in safe Rust, which is why it is the
Spacecraft Software default. It is also *not* a drop-in, despite being widely
described as one. Measured against jq 1.8.1:

- Nine jq command-line flags make jaq exit 2 with `unknown flag`.
- Twenty-two jq builtins are `undefined filter`.
- **jaq does not auto-vivify**: `null | .a = 1` and `{} | .a.b = 1` error, where
  jq builds the containers. This is the one that breaks real scripts, and it is
  not mentioned in any of the "drop-in replacement" write-ups.

`alias jq = jaq` hides all of that until it fails at runtime. Pathfinder closes
the gap where it can and says so precisely where it cannot.

## Status

| | |
|---|---|
| Baseline | jq **1.8.1** |
| Engine | jaq **3.0.0** |
| Differential suite | **108 invocations**, byte-identical stdout and exit code |
| Known unrepaired divergences | 7, all in [`doc/DIVERGENCES.md`](doc/DIVERGENCES.md) |

## Install

```sh
cargo build --release
./target/release/pathfinder --install-shim ~/.local/bin
```

That creates `~/.local/bin/jq` pointing at the binary. It refuses to overwrite an
existing `jq` without `--force`.

On a declaratively managed host, prefer adding the package to `home.packages` or
`environment.systemPackages` — `packaging/default.nix` installs the `jq` symlink
itself. Pathfinder never edits your shell configuration.

`jaq` must be on `PATH`, or named by `PATHFINDER_JAQ`.

## Use

Exactly as jq:

```sh
jq -r '.items[] | select(.active) | .name' data.json
jq -s 'add' a.json b.json
jq -n --jsonargs '$ARGS.positional' 1 '{"k":2}'
```

Under its own name it also answers two questions of its own:

```sh
pathfinder --explain -c '[fromstream(tostream)]'   # what would jaq receive?
pathfinder --install-shim ~/.local/bin [--force]
```

`--explain` prints the translated argv, the assembled program, which polyfills
were injected, whether the run will `exec` or pipe, and a warning if the filter
assigns to a path. These verbs are unavailable through the `jq` symlink on
purpose: a script passing `--explain` to what it believes is jq must get jq's
`Unknown option` error.

## How it works

```
argv[0] ──► parse jq's grammar ──► translate flags ──► exec jaq
              (hand-rolled)          (whitelist)        └── or spawn + pipe
                                                            for -a / --seq /
                                                            multi-file input
```

- **The fast path is `exec`.** The process image is replaced by jaq's; there is
  no second process and no copying. Cost over calling jaq directly: one
  `execve`. This is the path for nearly every invocation.
- **The filter is left byte-identical** unless it actually needs something —
  most filters are passed through untouched.
- **Polyfills are prepended on one line**, so jaq's reported line numbers still
  match what you typed. A `def` of your own for the same name wins over the
  injected one, because both jq and jaq take the last definition.
- **Unknown flags are rejected, never forwarded.** jaq has `-i/--in-place` and jq
  does not; forwarding would turn `jq -i '.' data.json` into a silent rewrite of
  your file.

## Scope

Pathfinder will not silently change what a filter means. Where jaq's semantics
differ from jq's in a way that cannot be repaired from the filter level — the
assignment operators, `*`, `/` — it documents and warns rather than guessing.

It also refuses to be *more* permissive than jq. `leaf_paths`, `ascii`,
`isvalid`, `toarray`, `ANY`, `ALL`, `@base32` and friends are missing from jaq,
but they are missing from jq 1.8.1 too, so Pathfinder does not define them.
Accepting a filter that real jq rejects would let a script pass here and break on
the next machine.

## Testing

```sh
make check      # fmt + clippy + tests
cargo test      # unit tests, plus the differential suite
```

The differential suite runs every case through **real jq** and through the shim
and compares bytes and exit codes. Real jq does not have to be installed: it is
located via `PATHFINDER_REAL_JQ`, or fetched ephemerally with
`nix build nixpkgs#jq`, and the suite skips if neither is available.

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
