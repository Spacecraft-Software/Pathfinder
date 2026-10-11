<!--
SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
SPDX-License-Identifier: CC-BY-SA-4.0
-->

# Divergences

Where Pathfinder cannot make `jaq` behave like `jq`, and why.

Everything here was measured, not inferred: **jq 1.8.1** (the declared baseline,
fetched with `nix build nixpkgs#jq`) against **jaq 3.0.0, 3.1.0 and 3.1.1**. The
cases Pathfinder *does* fix are mostly not listed here — they are in the
differential suite (`tests/differential.rs`), which asserts byte-for-byte
agreement on 156 invocations.

## Assignment, deletion and syntax: repaired by rewriting

Pathfinder parses the filter with a transcription of jq 1.8.1's grammar
(`src/syntax/`) and rewrites what jaq would run differently. Untouched text is
copied byte for byte; a filter that needs nothing is passed through unchanged.

| What | jq 1.8.1 | bare jaq | Pathfinder |
|---|---|---|---|
| `null \| .a.b = 1`, `.[2] = 1` on `[]` | builds the containers | error | jq's result |
| `.[1,2] \|= empty`, `del(.[0,1])` | deletes both | deletes the second against the shortened array | jq's result |
| `del(.a)`, `.a \|= empty` on `{"a":…,"b":…,"c":…}` | keeps key order | swaps the last key into the hole | jq's order |
| `. as [$a] ?// $b \| …` | destructuring alternatives | parse error | jq's semantics |
| `. as {$b: [$c]}` | binds and destructures | parse error | jq's result |
| `{(0): 1}`, `{(1+1): 2}`, `. as {(true): $x}` | compile error, exit 3 | accepted | jq's error, exit 3 |
| `module (.+1); 0`, `module []; 0` | compile error | accepted | jq's error |
| `.a?//1` | syntax error | `.a? // 1` | jq's error |
| `{(.a): 1}` with a non-string `.a` | run-time error | `{1:1}` — not JSON | jq's error |
| `{a, $__loc__}` | `"__loc__"` shorthand | parse error | jq's result |
| `reduce .[] / .[] as $x (…)` | source is a whole expression | parse error | parenthesised |
| `.[1.5]`, `.[length/2]`, `.[.i]` with `"i": 1.0` | truncates the index | error | jq's result |
| `.[nan]`, `.[1.2:3.5]`, `.[nan:2]` | `null`; bounds rounded outwards | error | jq's result |
| `null \| .[1:3]`, `[.[] \| .[1:3]?]` over nulls | `null` | error | `null` |
| `1 / 0`, `0 / 0`, `.a /= 0` | error: `… cannot be divided because the divisor is zero` | `Infinity`, `NaN` — **not JSON** | jq's error |
| `5.9 % 2.9`, `5 % 0.4`, `.a %= 3` | on integers: `1`; divisor truncates to 0, error | `0.1`; `NaN` | jq's integer remainder and error |
| `select(. > .5)` | `.5` is `0.5` | parse error | `0.5` |
| `[(1,2) + (10,20)]`, `[input - input]`, `try (.a + .b) catch .` | right operand in the outer loop: `[11,12,21,22]`; `.b`'s error | left: `[11,21,12,22]`; `.a`'s error | jq's order |

Measured at 100,000 elements, the rewritten assignments take between 0.1× and
3.6× jq's time (the slow end is building nested fields, `.[].w.x = 1`); `del`
takes between 0.6× and 2.9×.

Division and remainder are checked inline, with the right operand evaluated in
the outer loop as in jq: `1 / $i` over 300,000 values took 0.17 s (jaq 0.07 s,
jq 0.11 s), `100 % $i` 0.40 s (jaq 0.07 s); a divisor written as a non-zero
literal (`. / 7`, `. % 7`) costs nothing for `/` and 0.1 s for `%`.
`infinite % 1` is `NaN` here and `0` in jq, which clamps to a 64-bit integer
first.

jq evaluates the right operand of an arithmetic operator or a comparison
first, in the outer loop; jaq evaluates the left. It shows in the order of
outputs when both operands yield several values, in the order of effects
(`input - input`, `debug`) and in which error is raised when both fail
(`.a + .b` on a number raises `.b`'s). `+`, `-`, `*` and the comparisons are
rewritten to bind the right operand first, `(R) as $t | L op $t`, unless
either operand is a literal, a variable or `.` — then no order can be
observed, and the filter is left as written. The binding costs nothing
measurable: 2,000,000 rewritten operations took 2.9–3.3 s against jaq's own
2.8–3.2 s. `and`, `or` and `//` already agree.

An index whose key is not a literal (`.[$i]`, `.[.k]`) is rounded inline, which
costs about 0.5 µs per indexing: 300,000 of them took 0.49 s, against 0.30 s
unrewritten and jq's 0.22 s. A key written as an integer or a string is left
alone, and slices cost nothing measurable (0.37 s against 0.35 s).

What is still not quite jq:

- **Error wording on a bad path, on a stock jaq.** Where jaq's own `path()`
  fails, the message is jaq's (`invalid path expression with input …`); the
  packaged jaq gives jq's (see "Path expressions, on the packaged jaq").
- **Paths through a rounded index.** `path(.[1.5])` is `[1.5]` in jq and `[1]`
  here; `null | path(.[1:3])` is `[{"start":1,"end":3}]` in jq and `[]` here.
  The values read and written are jq's.
- **`.[infinite]`** is `null` in jq and still an error here; `.[nan] = 1`
  raises an error in both, but with different words.
- **`break` inside a `?//` body.** jq treats it like an error and tries the next
  alternative; the rewrite, built on `try`, lets it through.
- **`.[0]` of an object** is `null` in jaq and an error in jq. The `?//` rewrite
  checks for it, because there it changes which branch runs; elsewhere it does
  not.

`PATHFINDER_NO_REWRITE=1` hands every filter to jaq unparsed and unchecked.

## Not repaired, by design

| Case | jq 1.8.1 | jaq 3.0.0 | Why not |
|---|---|---|---|
| `"a" * 0`, `"a" * 0.5` | `""` | `null`, error | Fixed by the packaged jaq (see the number model); the rewriter leaves `*` alone. |
| `debug` output | `["DEBUG:",1]` | `["DEBUG:", 1]` | Fixed by the packaged jaq. stderr is never captured, so a stock jaq keeps its spacing. |
| An uncaught error | `jq: error (at <stdin>:3): …`, then the next input | `Error: …`, and stops | Fixed by the packaged jaq; see below. A stock jaq keeps its own report, for the same reason. What a `catch` handler sees *is* jq's text on both. |
| `halt_error` on a non-string | its JSON and a newline; nothing for `null` | no newline; `null` | Fixed by the packaged jaq. |
| `.[3]`, `.[3] = 1`, `has(3)`, `path(.[3])` on an object | `Cannot index object with number` | `null`, `{"a":1,3:1}` — **not JSON**, `false`, `[3]` | Fixed by the packaged jaq. Rewriting would put a check on every index in every filter. |
| Malformed input: `1 2 }` | `1`, `2`, `jq: parse error: Unmatched '}' at line 1, column 5` | `1`, `2`, `Error: failed to parse: value expected` | Fixed by the packaged jaq; see below. Input is outside the filter. |
| `try path(reverse \| .[0]) catch .` | `Invalid path expression near attempt to access element 0 of [1]` | `invalid path expression with input [1]` | Fixed by the packaged jaq; see below. jaq's path tracking is in its core. |

### Uncaught errors, on the packaged jaq

The packaged jaq reports an error that escapes the filter as jq 1.8.1 does —
`jq: error (at <stdin>:3): Cannot index number with string "a"`, or
`jq: error (at <stdin>:0) (not a string): {"a":1}` for a non-string error —
abandons the rest of that input's outputs, and goes on to the next input. Its
exit status is jq's: 5 when the last input failed, whatever came before, and
under `-e` the last input's result, else the last input with output's.

The location is jq's: `<stdin>` or the file name, then the number of complete
lines read when the error is reported. jq reads with `fgets`, up to a newline
or 4095 bytes at a time, so `1 2 3` with no newline reports `:0` for each
value, `1 \n2` reports `:1` for both, and a value followed by 4094 more bytes
on its line is reported a line earlier. Before any input is read, under `-n`,
it is `<unknown>`. The message
is translated by the same rules as a `catch` handler's (`jaq/src/jq.rs` in
`packaging/jaq/0008-uncaught-errors.patch`), so the two always agree.

Limits:

- With several input files, Pathfinder concatenates them onto jaq's stdin
  (see "Several input files are one stream" below) and describes them to the
  packaged jaq in `PATHFINDER_INPUTS` (names, byte counts, unreadable files),
  so the location names the file being read and restarts the count in each,
  as jq does. A stock jaq ignores the variable and reports `<stdin>:N`,
  counted across the files; it also shows the variable in `$ENV`, which the
  packaged jaq removes.
- A message outside the translation table keeps jaq's words in both places,
  for instance `test` on a number: `cannot use 1 as string`, where jq says
  `number (1) cannot be matched, as it is not a string`.
- When stdout and stderr are the same file, jq writes its outputs in blocks
  and jaq per value, so the two interleave differently. Each stream on its
  own is jq's.
- On stdin, a top-level `true`, `false` or `null` that ends exactly at a
  4095-byte boundary of a longer line is reported a line short: jaq's lexer
  does not read the byte after a literal, which jq's does.
- Counting lines costs nothing measurable on file input and on line-delimited
  stdin (14 MB of JSON lines: 3–5% *fewer* cycles than before); one 29 MB
  single-line document on stdin takes about 5–10% more.

### Malformed input, on the packaged jaq

The packaged jaq reads JSON as jq does and reports a malformed text in jq's
words, line and column: `jq: parse error: Unfinished JSON term at EOF at line
1, column 1`, exit 5, after the outputs before it. Under `-n`, `input` and
`inputs` raise the message as an uncaught error (`jq: error (at <stdin>:0):
Unmatched '}' at line 1, column 5`) that `try` can catch. `fromjson` gives
jq's `… (while parsing '…')`, `Expected JSON value` and `Unexpected extra JSON
values`; a bad `--argjson`, `--jsonargs`, `--slurpfile` or `--rawfile` gives
jq's message and exit 2. jaq's parser still builds the values; when it fails,
a transcription of jq's own parser (`jv_parse.c`) re-reads the value that
failed to find jq's message. To stop where jq stops, it now rejects what jq
rejects: `#` comments, `b"…"` byte strings, non-string object keys, a literal
or number run into the next token (`truex`, `1.5.`), and nesting deeper than
10000.

Limits:

- Numbers jq accepts and jaq's parser does not — `1.`, `.5`, `-.5` — are
  errors, reported as `<jaq's message> at line L, column C`.
- After a parse error no more values are read; jq discards its 4 KiB buffer
  and reads on, which shows only when `try input catch …` swallows one.
- `--seq` input reaches jaq without its RS bytes, so jq's `ignoring parse
  error` and `Truncated value` messages are not given.
- With several bad `--argjson`-family arguments, the first reported can
  differ: jaq checks them by kind, jq in argv order.
- Reading costs 2.5–7% more instructions than before these patches (29 MB
  indented on stdin: +7%, about 3 points of it rejecting `#` comments; 14 MB
  of JSON lines on stdin: +3%; either from a file: +3%). On stdin the text of
  the value being parsed is kept until it ends, about its size again for one
  huge document.

### Path expressions, on the packaged jaq

jq runs any filter inside `path()` and fails only when a lost path is used;
the packaged jaq does the same, with jq's messages: `Invalid path expression
with result [2,1,0]` at the end of `path()`, `… near attempt to access element
0 of [1]` or `… near attempt to iterate through …` at a later step, values cut
as jq cuts them. `[f]`, `-f` and a `catch` handler run as path expressions, as
in jq, so `path(null)` is `[]`; `path(.a[] // .b)` keeps only the true outputs
of `.a[]`, which jaq did not.

Limits:

- jq keeps a value's path while it is the *same* value; only `null`, `true`
  and `false` are recognised so here. `. as $x | path($x)` is `[]` in jq and
  an error here.
- Builtins that are jq definitions in jq and native in jaq (`getpath`,
  `ascii_downcase`, `limit`) fail at a different step, so the message names
  a different value; `|=` inside `path()` shows jq's internals there and not
  here.
- Keys in the message are shown after Pathfinder rounds them (`.[1.5]` reads
  `element 1`).
- `.[nan] = 9` still raises `Out of bounds negative array index`, where jq says
  `Cannot set array element at NaN index`: the rewriter folds a NaN key before
  jaq sees it.
- Update (`|=`, `=`) of a target jaq cannot update but jq can, such as
  `(try .a catch .b) |= 1`, keeps jaq's `invalid path expression with input`.

## Error messages a script can see

A script sees an error's text only through `try … catch`, and that is where
Pathfinder gives it jq's wording. jaq's generic messages carry the values
involved, so they are parsed back and re-described as jq does:

| jaq | jq 1.8.1 (and Pathfinder, in a `catch` handler) |
|---|---|
| `cannot use 123 as iterable (array or object)` | `Cannot iterate over number (123)` |
| `cannot index 1 with "a"`, `cannot index 0 with 0` | `Cannot index number with string "a"`, `Cannot index number with number` |
| `cannot calculate "a" - "a"` | `string ("a") and string ("a") cannot be subtracted` |
| `cannot calculate 1 % 0` | `number (1) and number (0) cannot be divided (remainder) because the divisor is zero` |
| `true has no length`, `cannot use "a" as number` | `boolean (true) has no length`, `string ("a") number required` |
| `cannot use "foo" as number` (from `-.`) | `string ("foo") cannot be negated` |

Values are shown as jq 1.8.1 shows them: cut to 11 bytes plus `...`, never
splitting a character. Builtins whose jq message names the builtin —
`utf8bytelength`, `trim`/`ltrim`/`rtrim`, `bsearch`, `mktime`, `strftime`,
`strflocaltime`, `has`, `implode`, `toboolean`, `tonumber` — raise jq's text
themselves.

Limits:

- A handler that never reads the error (`catch null`, `catch empty`, a
  constant) is left alone; translating costs about 5 µs per caught error when
  the message is not one of jaq's, and about 18 µs when it is.
- A user's own `error("cannot use 1 as number")` is translated too, since it
  cannot be told apart from jaq's.
- **Path-expression errors** keep jaq's words on a stock jaq; the packaged jaq
  reports them as jq does (see "Path expressions, on the packaged jaq").

## The number model

jq holds every number as a double, keeping a literal's text until it is
computed on; jaq has integers, floats and literals. The Nix package of
Pathfinder pins a jaq patched to close this gap (`packaging/jaq/`, small
patches applied to jaq 3.1.1; the later ones repair object keys, `debug`,
uncaught errors, `halt_error`, path expressions, `%`, malformed input and
file locations). The table shows both: what a stock jaq gives,
and what the packaged one gives.

| Filter | jq 1.8.1 | stock jaq | packaged jaq |
|---|---|---|---|
| `4 / 2`, `[1,2,3] \| add / length`, `1.5 * 2`, `9 \| sqrt` | `2`, `2`, `3`, `3` | `2.0`, `2.0`, `3.0`, `3.0` — also in `tostring`, `tojson` | as jq |
| `1e3`, `"1e3" \| tonumber` | `1E+3` | `1e3` | as jq |
| `1e17 * 1`, `1e-7 * 1` | `1e+17`, `1e-07` | `1e17`, `1e-7` | as jq |
| `nan`, `infinite` | `null`, `1.7976931348623157e+308` | `NaN`, `Infinity` — **not JSON** | as jq |
| `0 * -1` | `-0` | `0` | as jq |
| `9007199254740993 * 1` | `9007199254740992` (a double) | `9007199254740993` (exact) | as jq |
| input `[nan, -Infinity, inf]`, a leading byte-order mark | read | error | as jq |
| `"a" * 0`, `"a" * 0.5`, `3.7 * "a"`, `"a" * 1e9` | `""`, `""`, `"aaa"`, error | `null`, error, error, an allocation | as jq |
| `[(infinite, -infinite) % (1, -1, infinite)]`, `1e19 % 7` | `[0,0,0,0,0,-1]`, `0` | `[NaN,…]`, `3.0` | as jq: both sides clamped to 64-bit integers |

The packaged jaq prints a decimal literal the way jq's decNumber does (`1E+3`,
`1.000`, `1E-7`), computes integers beyond 2^53 in doubles as jq does, and is
no slower: printing 500,000 objects took 2.8–3.1 s against stock jaq's
3.3 s (jq: 4.2 s), measured interleaved on a loaded host. Its `--version`
reads `jaq 3.1.1+pathfinder.3`, and the conformance suite holds it to its own
ratchet, `tests/jq-suite/FLOOR-patched`.

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

On the packaged jaq this costs nothing, because the exit codes agree: `-e` on
`false`/`null` → 1, `-e` with no output → 4, `halt_error` → 5, `halt_error(7)` →
7, compile error → 3, usage error → 2, and malformed JSON input → 5 in **both**
(an earlier draft of this document claimed jq exits 2 here; that was wrong).
A stock jaq stops at the first uncaught error and exits 5, where jq goes on
to the next input and exits with the last input's status: `1 {}` through
`.a` exits 0 in jq.

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
| `match` on a group that did not take part, `"b" \| match("(a)?b")` | leaves the group out of `captures` | lists it: `{"offset":-1,"string":null,"length":0,"name":null}` |
| `match` on an unnamed group | no `name` key | `"name": null` |
| `capture("(?<x>a)?b")` on `"b"` | `{}` | `{"x":null}` |
| `match("[a-z]*"; "g")` on `"ab1"` | skips the empty match at 2 | reports it, as Oniguruma does |
| `gsub("(?<x>.)"; "\(.x)", "-")` (several outputs) | the cartesian product | jq's one string per output |
| `ltrimstr`/`rtrimstr`/`startswith`/`endswith` on a non-string | generic error | jq's own message |
| `setpath` past an array's start / at a huge index | pads or errors oddly | `Out of bounds negative array index` / `Array index too large` |
| `delpaths` | deletes in the order given; reorders object keys | jq's simultaneous deletion, keys in order |
| `tonumber` | parses a JSON stream: `"1a"` → `1` then an error, `" 4"` → `4`, `""` → nothing, `"+5.43"` → `+5.43` (not JSON) | jq's grammar: sign, digits, point, exponent, `nan`, `infinity`; anything else is an error |
| `from_entries` | reads only `key`/`k`/`name`, `value`/`v`; `[{"key":null,"value":1}]` → `{null:1}` — not JSON | also `Key`/`Name`/`Value`; a non-string key is an error |
| `with_entries(f)` | goes through jaq's own `from_entries` | through the repaired one |
| `has(k)` | `true` for a negative index, `false` for a number key on an object; errors on `has(nan)`, `has(1.5)` | `false`, an error, `false`, truncates; `null` has nothing |
| `implode` | rejects `1.5`, `-1`, `1114112` | truncates; writes U+FFFD outside Unicode and for surrogates; jq's messages |
| `toboolean` | parses JSON: `" true"` → `true` | exactly `true`, `false`, `"true"`, `"false"` |
| `@base64d` | exact padding only; rejects unused low bits (`"QR=="`) | decodes up to the first `=`, padding optional, low bits ignored; jq's two errors |
| `@urid` | keeps `%`-garbage as text; invalid UTF-8 becomes U+FFFD | both are errors |

The regex repairs need to know which capture groups can go unmatched and
whether a regex can match empty. For regexes written as literals in the filter
that is worked out once, in `src/regex.rs`; a regex computed at run time is
scanned on every call. Measured on 100,000 lines (pre-repair time in brackets,
jq 1.8.1 in parentheses): `match` with a group 2.6 s [2.1 s] (0.8 s), `capture`
2.9 s [2.5 s] (1.4 s), `sub` 2.8 s [2.1 s] (2.5 s), `gsub("[0-9]"; "#")`
13.2 s [16.2 s] (24.6 s), `scan` 3.6 s [1.8 s] (1.0 s). A group that can go
unmatched takes the slower general path (`capture` with an optional group:
5.7 s). Lookaround and `\b`'s Unicode word rules belong to the regex engine and
stay jaq's.

One deliberate exception: jq 1.8.1's `@urid` turns every non-ASCII character
of its input into U+FFFD (`"é%41" | @urid` is `"��A"`), a bug jq 1.8.2 fixed
(`"éA"`). Pathfinder follows 1.8.2 there rather than reproduce the bug.

## Behaviour Pathfinder adds on top of jaq

Not divergences from jq — these are places where jaq differs from jq and
Pathfinder papers over it, listed so the mechanism is visible:

- **Several input files are one stream.** jq concatenates them; jaq runs the
  whole program once per file, so `jq -s . a.json b.json` yields two arrays under
  bare jaq instead of one. Pathfinder feeds the files to jaq as a single
  concatenated stdin stream, with nothing between files, as jq reads them
  (`12` then `34` is `1234`). This costs the `exec` fast path, so it only
  happens when there really is more than one input. The files are opened
  before jaq starts, and a file that is not regular (stdin `-`, a FIFO, a
  process substitution) is read into memory first unless it is the last,
  because the packaged jaq needs each file's length up front.
- **An unreadable input file** (among several) is reported by the packaged
  jaq when its reader reaches it, `jq: error: Could not open file x: No such
  file or directory` (or `jq: error: Is a directory`); jq then runs one more
  input and exits 2. On a stock jaq, Pathfinder prints the same message before
  jaq starts, skips the file, and exits 2. Under `--seq` jq's own reaction to
  a missing file is stranger (it reports a parse error and prints nothing);
  that is not reproduced. A *single* unreadable file is still jaq's
  `Error: x: No such file or directory (os error 2)`, exit 2.
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
