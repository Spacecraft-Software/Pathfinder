<!--
SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
SPDX-License-Identifier: CC-BY-SA-4.0
-->

# Divergences

Where Pathfinder cannot make `jaq` behave like `jq`, and why.

Everything here was measured, not inferred: **jq 1.8.1** (the declared baseline,
fetched with `nix build nixpkgs#jq`) against **jaq 3.0.0**. The cases Pathfinder
*does* fix are not listed here — they are in the differential suite
(`tests/differential.rs`), which asserts byte-for-byte agreement on 108
invocations.

## The one that will actually bite you: auto-vivification

**jaq does not create missing containers when you assign through them.**

| filter | input | jq 1.8.1 | jaq 3.0.0 |
|---|---|---|---|
| `.a = 1` | `null` | `{"a":1}` | error |
| `.a.b = 1` | `{}` | `{"a":{"b":1}}` | error |
| `.a[0] = 1` | `{}` | `{"a":[1]}` | error |
| `.[2] = 1` | `[]` | `[null,null,1]` | error |
| `reduce … (null; .[$k] = 1)` | — | builds the object | error |

The error reads `cannot use null as iterable (array or object)`.

**What Pathfinder repairs.** Calls to `setpath` are shadowed with an
auto-vivifying definition that matches jq exactly — including the cases where jq
*errors* because the existing value is the wrong type, which a naive widening
version would silently accept. So `setpath(["a","b"];1)` on `null` works.

**What it cannot repair.** The `=`, `|=`, `+=`, `//=` operators. They are syntax,
not builtins, so no definition can shadow them; rewriting them means parsing jq
expressions to find the assignment's left- and right-hand sides. That is a
substantially larger project than the rest of this shim put together.

**Working around it.** Seed the container: `.a //= {} | .a.b = 1`, or use
`setpath`, which is repaired.

`pathfinder --explain '<filter>'` warns when a filter contains an assignment.

## Not repaired, by design

| Case | jq 1.8.1 | jaq 3.0.0 | Why not |
|---|---|---|---|
| `"a" * 0` | `""` | `null` | `*` is an operator; same problem as assignment. |
| `"a" * 0.5` | `""` | error | Same. |
| `1 / 0` | error | `Infinity` — **invalid JSON on stdout** | Same. Worth knowing about: the output will not parse. |
| `1e1000` | `1E+1000` | `1e1000` | Number rendering, not semantics. |
| `"aGk" \| @base64d` (unpadded) | lenient | `Invalid padding` | `@`-formats cannot be defined in the jq language. |
| `debug` output | `["DEBUG:",1]` | `["DEBUG:", 1]` | Stderr only. Fixing it means capturing stderr, which costs more than the space it saves. |
| Error text | `jq: error (at <stdin>:0): …` | `Error: …` | Same reason. stderr is passed through untouched so `debug`, colour, and interleaving stay correct. |

## Unsupported, with a diagnostic

Pathfinder refuses these by name rather than letting jaq produce a vaguer error.

- **`--stream` / `--stream-errors`.** jaq has no streaming parser. For a filter
  that only needs the events, `tostream` is semantically equivalent (it
  materialises the input first); for a file too large to hold in memory there is
  no substitute.
- **`input_line_number`.** jaq tracks no input position.
- **`modulemeta`, `get_search_list`, `get_jq_origin`, `get_prog_origin`.** jaq's
  module system exposes no introspection.
- **`lgamma_r`.**
- **`input_filename` with more than one input file.** With a single input, or
  with stdin, it is answered exactly. Across several files jq's answer changes as
  it advances, and any constant would be wrong for some of them — so it errors
  rather than answering quietly and wrongly.

## Names that are *not* polyfilled, deliberately

`leaf_paths`, `ascii`, `isvalid`, `toarray`, `ANY`, `ALL`, `GROUP_BY`,
`UNIQUE_BY`, `@base32`, `@base32d`.

These are absent from jaq — and **also absent from jq 1.8.1**. Several existed in
jq 1.6 and were removed since. Defining them would make Pathfinder accept filters
that real jq rejects, so a script that worked here would break on the next
machine. That is the worst failure mode a compatibility shim can have, and it is
worse than the missing name.

If you need them, define them in your filter; your definition wins over anything
Pathfinder injects.

## Consequences of the `exec` fast path

When no output transform is needed, Pathfinder replaces its own process with
jaq's. It therefore never sees jaq's exit code or stderr and cannot retouch
either.

In practice this costs nothing, because the exit codes already agree: `-e` on
`false`/`null` → 1, `-e` with no output → 4, `halt_error` → 5, `halt_error(7)` →
7, compile error → 3, usage error → 2, and malformed JSON input → 5 in **both**
(an earlier draft of this document claimed jq exits 2 here; that was wrong).

## Things that needed no work at all

Recorded because it is useful to know where the two already agree, and because
each was checked rather than assumed:

- `--unbuffered` — jaq already flushes per value on a pipe.
- `-b` / `--binary` — the option body is `#ifdef WIN32` inside jq itself.
- `--indent 0` — pretty-prints with zero indentation in both. It does **not**
  mean compact.
- `--args` and `$ARGS.named` — jaq's are position-sensitive and populated
  exactly as jq's are. Only `--jsonargs` needed emulation.
- `ltrimstr` on a non-string — jq 1.8.1 errors too. This was a real difference
  in jq ≤ 1.7, and is not one now.
- Literal-number preservation — jaq round-trips
  `100000000000000000000000000001` and `1.0000000000000000005` unchanged, and
  keeps them exact through arithmetic where jq falls back to a double and prints
  `1e+29`.

## Builtins repaired to jq's definitions

jaq has these names, but they behave differently. Pathfinder shadows each with
jq 1.8.1's own definition, or wraps jaq's builtin, only when the filter uses it:

| Builtin | jaq 3.1 | jq 1.8.1 (and Pathfinder) |
|---|---|---|
| `scan("c")` | first match only | every match (`scan` is always global) |
| `nth(1,2; g)` | one result | one result per index |
| `mktime`, `strftime`, `strflocaltime`, `todate` | reject a short array like `[2024,2,15]` | pad missing fields with zeros |
| `limit(-1; …)`, `skip(-1; …)`, `flatten(-1)` | silently empty / everything | error |
| `join(",")` with `null` items | writes `null` | writes nothing |
| `pick(.[1])` | `{1:2}` — not JSON | `[null,2]` |
| `match`/`test`/`capture` with `[re, flags]` | error | accepted |
| `ltrimstr`/`rtrimstr`/`startswith`/`endswith` on a non-string | generic error | jq's own message |
| `setpath` past an array's start / at a huge index | pads or errors oddly | `Out of bounds negative array index` / `Array index too large` |

Still divergent: jaq's `from_entries` (and any object construction with a
computed key) accepts a non-string key and prints an object that is not JSON
(`{null:2}`); jq errors.

## Behaviour Pathfinder adds on top of jaq

Not divergences from jq — these are places where jaq differs from jq and
Pathfinder papers over it, listed so the mechanism is visible:

- **Several input files are one stream.** jq concatenates them; jaq runs the
  whole program once per file, so `jq -s . a.json b.json` yields two arrays under
  bare jaq instead of one. Pathfinder feeds the files to jaq as a single
  concatenated stdin stream. This costs the `exec` fast path, so it only happens
  when there really is more than one input.
- **Output-format flags are last-wins.** `jq -c --tab` pretty-prints with tabs
  and `jq --tab -c` is compact; jaq lets `-c` win either way. Pathfinder forwards
  only the winning directive.
- **`-a` cancels `-r`.** `jq -ra '"café"'` prints `"café"`, quotes included.
  Pathfinder strips the raw flags from jaq's argv and re-applies the framing
  itself.
- **`--seq` is json-seq in both directions.** jq requires RS-framed input as well
  as producing it. Pathfinder strips RS on the way in and adds it on the way out.
- **`-Ldir`** — jq accepts the attached spelling, jaq rejects it. Always split.
- **`--arg` duplicates** — jq keeps the *first* binding of a repeated name;
  jaq's parser keeps the last. Pathfinder de-duplicates before forwarding.
- **`--argfile`** — removed in jq 1.8.1, so Pathfinder rejects it too rather than
  supporting something the baseline does not.
- **A leading `-` is data where jq says it is.** jaq reads any argument
  starting with `-` as a flag, so `jq -1`, a file named `-5`, and
  `jq --args a -5` all fail on bare jaq. Pathfinder disguises each one (a
  leading space, `./-5`, a by-name binding).
- **Unknown flags are rejected, never forwarded.** This is a safety property:
  jaq has `-i/--in-place` and jq does not, so a pass-through shim would turn
  `jq -i '.' data.json` from an error into a silent, irreversible rewrite of the
  user's file.
