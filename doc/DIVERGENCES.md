<!--
SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
SPDX-License-Identifier: CC-BY-SA-4.0
-->

# Divergences

Where Pathfinder does not behave as jq 1.8.1, and why.

Everything here was measured against **jq 1.8.1's release binary** (the
declared baseline; see `AGENTS.md` for how to fetch it), not inferred. What
Pathfinder *does* reproduce is not listed: the differential suite
(`crates/pathfinder/tests/differential.rs`) asserts byte-for-byte agreement
with jq on 1 191 invocations, stdout and exit status, and a part of it stderr
too; jq's own test suite passes 820 of 820.

Until 0.2.0 Pathfinder could also run on a jaq binary (`PATHFINDER_ENGINE=jaq`),
and most of this document described repairing jaq. That engine is gone; the
engine Pathfinder runs is its own, in process, and the repairs became the
engine's behaviour. `PATHFINDER_ENGINE` set to anything but `native` now
exits 2.

## Refused, by name

Pathfinder refuses these with a diagnostic naming the construct (exit 2),
rather than answering differently.

- **`--stream` / `--stream-errors`.** The engine has no streaming parser yet
  (PLAN P-075). For a filter that only needs the events, `tostream` is
  semantically equivalent; it reads the whole input first.
- **`modulemeta`, `get_search_list`, `get_jq_origin`, `get_prog_origin`.** The
  module system exposes no introspection.
- **`lgamma_r`.**
- **Oniguruma's subexpression calls (`\g<n>`, `(?&name)`, `(?R)`), absent
  operator (`(?~…)`) and callouts (`(*FAIL)`, `(?{…})`)**, which jq's
  Oniguruma accepts: `pathfinder-regex` refuses them at compile time with
  `Regex failure: subexpression calls, absent groups and callouts are not
  supported` (TODO T-044).

## Paths and updates

- **A value's path.** jq keeps a value's path while it is the *same* value, so
  `. as $x | path($x)` is `[]`; here it is `Invalid path expression with
  result 1`.
- **An update through `try … catch`.** `(try .a catch .b) |= 1` updates `.a` in
  jq; here it fails with `invalid path expression with input …`.
- **A path through `?//`.** `path(.[] as [$a] ?// $a | .)` is `[]` for each
  element in jq; here it fails with `Invalid path expression with result …`,
  because the alternatives are tried with `try … catch`.
- **`break` inside a `?//` alternative.** jq treats it as an error and tries the
  next alternative: `[label $o | .[] as [$a] ?// $a | if $a == 1 then break $o
  else $a end]` on `[[1],2]` is `[[1],2]` in jq and `[]` here.

## Regular expressions

- **The retry limit.** Oniguruma gives up a match after 10 000 000 steps of
  its own (`retry-limit-in-match over`); `pathfinder-regex` counts its
  backtracks instead, and of Oniguruma's search optimisation it has only the
  required-literal check (TODO T-044). A pattern that backtracks without end
  on its text, such as `.{0,9}{0,9}.++++.*`, therefore can fail here where jq
  answers, or answer after seconds where jq rules the text out at once
  (`.{4}{3,}{3}?+…` on a text too short for it). Found by fuzzing; both give
  the same answer on the rest of what was found.

## Input and output

- **stdout and stderr in one file.** jq writes its outputs in blocks, the
  engine per value, so when both streams go to the same file their lines
  interleave differently (`1 2 | ., debug`, a parse error after the first
  value). Each stream on its own is jq's.
- **`--seq` input.** The input's RS separators are stripped before the reader
  sees them, so jq's `ignoring parse error: …` and `Truncated value` messages
  are not given; a malformed text is a plain parse error. With a missing file
  among several under `--seq`, both report the file; jq then also reports a
  parse error and prints none of the other files' values.
- **Several bad `--argjson`, `--jsonargs`, `--slurpfile` or `--rawfile`
  arguments.** The first one reported can differ: jq checks them in argv
  order, the engine by kind.

## Dates and times

The time builtins are glibc 2.39's code in Rust (M23), measured with
`tools/time-diff.py`: 0 of 100 000 random calls differ from jq under `UTC`,
`Asia/Kolkata`, `America/New_York` and `Europe/Berlin`. Two places do not
follow jq, deliberately:

| Case | jq 1.8.1 | Pathfinder |
|---|---|---|
| A local time past about year 5 881 600 (`1e15 \| localtime` in New York) | glibc's daylight saving rule overflows an `int` and stops applying | the zone's rule goes on applying |
| `strflocaltime` of a number `localtime` cannot convert (`1e17`) | aborts on an assertion, exit 134 | `localtime`'s error, `error converting number of seconds since epoch to datetime`, exit 5 |

The locale is C, as on a host without glibc's locale files, where jq's static
binary finds no other: `%c` is `Thu Mar  5 23:53:41 2015` whatever `LANG`
says.

## A jq bug not reproduced

jq 1.8.1's `@urid` turns every non-ASCII character of its input into U+FFFD
(`"é%41" | @urid` is `"��A"`), a bug jq 1.8.2 fixed (`"éA"`). Pathfinder
follows 1.8.2.

## Names that are *not* defined, deliberately

`leaf_paths`, `ascii`, `isvalid`, `toarray`, `ANY`, `ALL`, `GROUP_BY`,
`UNIQUE_BY`, `@base32`, `@base32d`.

These are **absent from jq 1.8.1** (several existed in jq 1.6 and were removed
since). Defining them would make Pathfinder accept filters that real jq
rejects, so a script that worked here would break on the next machine. That is
the worst failure mode a compatible jq can have, and it is worse than the
missing name. If you need them, define them in your filter.

## Performance

Not a divergence, but recorded with them (PLAN P-079). Measured 2026-10-31
on the dev host with `hyperfine -N -w 1 -r 5`, mean wall time, the three
binaries interleaved in one session: jq 1.8.1's release binary, Pathfinder
0.2.0 (`cargo build --release`), and a stock jaq 3.1.2 for reference. Inputs:
a 30 MB document of 100 000 objects (`doc`), the same as 30 MB of JSON lines
(`lines`), a 4 MB document (`small`), 100 000 short strings, and a 120 KB
text.

| Workload | jq 1.8.1 | Pathfinder | jaq 3.1.2 |
|---|---:|---:|---:|
| `-c .` on `doc` | 1731 ms | 945 ms | 928 ms |
| `select(.active) \| .id` on `lines` | 816 ms | 704 ms | 752 ms |
| `reduce range(3000000) as $i (0; . + $i)` | 935 ms | 835 ms | 968 ms |
| `.items \| group_by(.user)` on `small` | 185 ms | 78 ms | 80 ms |
| `.items[].score \|= . * 2` on `small` | 228 ms | 64 ms | 67 ms |
| `[.[] \| select(test("^[a-z]+[0-9]+$"))]` on the strings | 586 ms | 668 ms | 662 ms |
| `gsub("a"; "b")` on the text | 9766 ms | 124 ms | 1303 ms |
| `[range(300000) \| todate]` | 385 ms | 368 ms | 89 ms |
| `-n 1` (process start) | 1 ms | 2 ms | 2 ms |

Pathfinder is as fast as jq or faster everywhere but `test` over many short
strings (14% slower; not yet profiled) and process start-up
(about 1 ms, the engine's definitions compiled on each run). jq's global
regex filters are quadratic (its `f_match` counts every offset from the
start of the string); Pathfinder's give the same answers in linear time.
jaq's faster `todate` is not jq's: it formats with `jiff`, which Pathfinder
replaced with glibc's code to answer as jq does.
