<!--
SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
SPDX-License-Identifier: CC-BY-SA-4.0
-->

# CREDITS

Pathfinder exists to stand in for two programs it did not write, and it is
built directly on their observable behaviour. Standard §15.3 asks for this
acknowledgement to be explicit rather than left to SPDX metadata.

## jq — Stephen Dolan and contributors

<https://github.com/jqlang/jq> · MIT

Pathfinder reproduces jq's command-line grammar, its option semantics, its exit
codes, and its diagnostic wording. The argument-parsing rules in
`crates/pathfinder/src/jqargs.rs` are transcribed from jq's `src/main.c` (`isoptish`, `isoption`,
and the option loop), and `crates/pathfinder/src/embed/jq-help.txt` is jq 1.8.1's `--help` output
reproduced verbatim so that `jq --help` through the shim answers exactly as jq
does. The polyfill definitions in `crates/pathfinder/src/prelude.rs` are taken from jq's
`src/builtin.jq`, adapted only where jaq's semantics required it — each such
adaptation is noted at the definition.

`crates/pathfinder/tests/jq-suite/` vendors jq 1.8.1's own test files (`jq.test`, `man.test`,
the regex, base64 and URI suites) unchanged. They are the conformance measure:
written by jq's maintainers, they judge Pathfinder by a standard its author did
not choose.

The behavioural baseline is **jq 1.8.1**. Every golden value in the compat
corpus was produced by running that version.

## jaq — Michael Färber and contributors

<https://github.com/01mf02/jaq> · MIT

jaq is the engine Pathfinder hands off to today; Pathfinder's translation
layer in front of it implements none of the jq language itself. The
divergences recorded in `doc/DIVERGENCES.md` are observations about jaq's
behaviour, not criticisms of it — jaq does not claim byte-compatibility with
jq, and several of its choices (strict arithmetic, no auto-vivification) are
defensible on their own terms.

`packaging/jaq/` carries fifteen patches to jaq 3.1.1's own source, applied
when the Nix package builds the engine it pins. Among them: number output in
jq's format, jq's double-precision arithmetic above 2^53, jq's spellings of
NaN and infinity in input, a leading byte-order mark, jq's string repetition, object keys, `%`,
`halt_error`, path errors, and jq's wording and locations for uncaught and
parse errors. They are derived from jaq's code and keep its MIT licence
(Standard §4.2). They are carried in-tree rather than sent upstream (§6.4):
they make jaq behave like jq, which is Pathfinder's goal and not necessarily
jaq's.

### The engine crates

From M18 on, Pathfinder's engine is a hard fork of jaq. `crates/pathfinder-core`,
`crates/pathfinder-std`, `crates/pathfinder-json` and `crates/pathfinder-fmts`
are jaq 3.1.1's `jaq-core`, `jaq-std`, `jaq-json` and `jaq-fmts` (upstream tag
`v3.1.1`, commit `c866e70303b5dbc37d83a0b0cbacf10e90af9c8c`), with the library
hunks of the patches above applied. The interpreter, the standard library, the JSON value type and
its reader and writer are jaq's work: the compiler from jq syntax to jaq's
filter representation, the lazy evaluation of paths and updates, and the
definitions in the `.jq` files all come from it. `crates/UPSTREAM.md` records
the import base, its hash, and every change since.

The imported files keep jaq's MIT licence and its copyright statement,
"Michael Färber" as the crates' `authors` field gives it, in `REUSE.toml`;
jaq ships no per-file headers and no copyright line in its `LICENSE-MIT`, so
the licence text is the one in `LICENSES/MIT.txt`. Fixes from later jaq
releases are ported by hand; nothing goes back upstream (§6.4).

---

*--- Forged in Spacecraft Software ---*
