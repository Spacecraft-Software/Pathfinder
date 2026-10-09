<!--
SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
SPDX-License-Identifier: CC-BY-SA-4.0
-->

# Pathfinder — Plan

MVP: M10–M13 — Step 3: about 95% of jq's suite

Steps 1 and 2 (PRs #5 and #6) took jq 1.8.1's own test suite from 80.9% to
89.1% (731/820). This plan covers the next proposals, numbered on from step 2's
M7–M9. Every expected value comes from running real jq 1.8.1; every cost
figure is a measurement under jaq 3.1.0 at the size stated.

The measured starting point, by cause of the 89 remaining failures:

| Cause | Cases | Milestone |
|---|---|---|
| Error message wording | ~35 | M10 |
| Fractional, NaN and null indices | ~14 | M11 |
| Regex results | ~12 | M12 |
| Accepting input jq rejects | ~9 | M13 |
| Division by zero, NaN, Infinity | ~8 | Phase 2 |
| Number printing, big integers, input parsing | ~9 | Phase 2 |
| Unreachable (decNumber literals, jq quirks) | ~5 | — |

Phase 2 (proposals 5 and 6) is planned once M10–M13 have landed.
Order of work: M13, M11, M12, M10 — the most mechanical first, and the
message translation, which is the most fragile, last.

## M10 — jq's error messages

jaq's errors carry different text (`cannot use 123 as iterable (array or
object)` where jq says `Cannot iterate over number (123)`). Scripts see the
text only through `try … catch`, so that is where it is translated; a builtin
whose jq message names the builtin gets a guard that raises jq's text itself.
A user's own `error("…")` whose text happens to match one of jaq's
templates is translated too; that collision is contrived and documented.
Both cost nothing until an error happens, apart from the guard's `try`
(0.3–0.7 µs per call, measured on `keys`, `sort_by` and `test`).

- [x] P-001 `_pf_dump`: jq 1.8.1's truncated value dump (11 bytes, UTF-8 safe)
- [x] P-002 `try … catch` handlers see jq's wording for iterate, index, arithmetic, length and number errors
- [x] P-003 Unary minus on a non-number raises `cannot be negated`
- [x] P-004 Builtin guards raise jq's own message, only when the input type is wrong
- [x] P-005 Differential cases for every translated message

## M11 — Fractional and null indices

jaq rejects `.[1.5]`, `.[1.0]` from JSON data, `.[length/2]` and fractional
slice bounds, and errors on `null | .[1:3]`; jq truncates, rounds slice bounds
outwards and answers `null`. A non-literal index key is normalised inline
(nested `if`, measured +0.4 µs per index; a `type ==` test cost three times
that), so it stays a path expression for assignment and `del`.

- [x] P-006 Array index keys truncate toward zero, as jq's `jv_get`; literal fractions too
- [x] P-007 Slice bounds round start down and end up; NaN bounds; `null` slices
- [x] P-008 Index-heavy benchmark before and after, recorded in DIVERGENCES

## M12 — Regex results like jq's

jaq leaves unmatched groups out of `captures` and omits `"name": null`, so
`capture` drops keys jq reports as `null`, and its `sub`/`gsub` differ on
empty matches and on a replacement with several outputs. Groups are given
synthetic names before matching, so every group can be reported in place.

- [x] P-009 `match` reports every group: `name`, and `offset: -1` when unmatched
- [x] P-010 `capture` reports unmatched named groups as `null`
- [x] P-011 `sub`/`gsub` are jq 1.8.1's definitions over the repaired `match`
- [x] P-012 Regex benchmark before and after
- [ ] ~~P-018 Lookaround and Unicode `\b`~~ (dropped) — regex engine, phase 2

## M13 — Reject what jq rejects

- [x] P-013 `@base64d` raises jq's errors on invalid input
- [x] P-014 `@urid` raises jq's error on an invalid encoding
- [x] P-015 `implode` follows jq's codepoint rules and messages
- [x] P-016 `from_entries` is jq 1.8.1's definition
- [x] P-017 `has(nan)`, `toboolean`, `trim` and the remaining builtin edge cases
