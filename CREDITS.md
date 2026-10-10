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
`src/jqargs.rs` are transcribed from jq's `src/main.c` (`isoptish`, `isoption`,
and the option loop), and `src/embed/jq-help.txt` is jq 1.8.1's `--help` output
reproduced verbatim so that `jq --help` through the shim answers exactly as jq
does. The polyfill definitions in `src/prelude.rs` are taken from jq's
`src/builtin.jq`, adapted only where jaq's semantics required it — each such
adaptation is noted at the definition.

`tests/jq-suite/` vendors jq 1.8.1's own test files (`jq.test`, `man.test`,
the regex, base64 and URI suites) unchanged. They are the conformance measure:
written by jq's maintainers, they judge Pathfinder by a standard its author did
not choose.

The behavioural baseline is **jq 1.8.1**. Every golden value in the compat
corpus was produced by running that version.

## jaq — Michael Färber and contributors

<https://github.com/01mf02/jaq> · MIT

jaq is the engine Pathfinder hands off to; Pathfinder is a translation layer in
front of it and implements none of the jq language itself. The divergences
recorded in `doc/DIVERGENCES.md` are observations about jaq's behaviour, not
criticisms of it — jaq does not claim byte-compatibility with jq, and several of
its choices (strict arithmetic, no auto-vivification) are defensible on their
own terms.

`packaging/jaq/` carries five small patches to jaq 3.1.1's own source, applied
when the Nix package builds the engine it pins: number output in jq's format,
jq's double-precision arithmetic above 2^53, jq's spellings of NaN and infinity
in input, a leading byte-order mark, and jq's string repetition. They are
derived from jaq's code and keep its MIT licence (Standard §4.2), and they are
carried in-tree rather than sent upstream (§6.4) — they make jaq behave like
jq, which is Pathfinder's goal and not necessarily jaq's.

---

*--- Forged in Spacecraft Software ---*
