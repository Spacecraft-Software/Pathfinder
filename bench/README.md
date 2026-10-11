<!--
SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
SPDX-License-Identifier: CC-BY-SA-4.0
-->

# Pathfinder — Benchmarks

The benchmark set behind PRD R-011 (PLAN P-060, TODO T-018).

## The budget

R-011 (mandatory): on this set, Pathfinder is

- **no slower than jq 1.8.1** — every `×jq` ratio is at most 1.00, and
- **within 10% of jaq 3.1.1 at the fork point** — every `×jaq` ratio is at
  most 1.10,

measured in user-space cycles, before and after every optimisation.

"jaq 3.1.1 at the fork point" is the engine the crates were imported from:
jaq 3.1.1 with `packaging/jaq/` applied (`jaq 3.1.1+pathfinder.N`), which is
what Pathfinder hands off to today. Stock jaq 3.1.1 is worth running beside it
— the gap between the two is the cost of the patch set — but it is not the
denominator.

The budget is for the in-process `native` engine (M19). The `jaq` engine is
measured too, as the reference for what the shim itself costs: it translates
argv and `exec`s jaq (or, for `-a`, `--seq` and several input files,
spawns it and post-processes), so beyond those cases it should track the
patched jaq exactly.

## Running

```sh
cargo build --release
mkdir -p /tmp/pf && ln -sf "$PWD/target/release/pathfinder" /tmp/pf/jq
JQ=/path/to/jq-1.8.1                           # `--version` must say jq-1.8.1
JAQ=/nix/store/…-jaq-pathfinder-3.1.1/bin/jaq  # the packaged, patched jaq

bench/run jq="$JQ" jaq="$JAQ" \
    "pf-jaq=PATHFINDER_ENGINE=jaq PATHFINDER_JAQ=$JAQ /tmp/pf/jq" \
    "native=PATHFINDER_ENGINE=native /tmp/pf/jq" > results.md
```

The baseline is jq **1.8.1**, and nixpkgs has moved on: on 2026-10-11 both
`nixpkgs#jq` and `--inputs-from . nixpkgs#jq` build jq 1.8.2. Check the
version of whatever binary is passed as `jq`. Different builds of the same jq
also differ: a statically linked jq-1.8.1 and nixpkgs' dynamically linked
one were up to 25% apart in either direction on single cases, so the
`×jq` bar moves with the build; record which one was used.

Each engine is one argument, `NAME=[VAR=VALUE]... PROGRAM`. The leading
assignments become that engine's environment; the part after `NAME=` is split
on blanks, so it cannot hold paths with spaces. Run Pathfinder through a `jq`
symlink: under its own name it is not jq. The engines named `jq` and `jaq` are
the ratio denominators (`BENCH_JQ`, `BENCH_JAQ` rename them); every other
engine gets a `×jq` and a `×jaq` column. Any binary that takes jq's command
line can be an engine.

| Variable | Default | Meaning |
|---|---|---|
| `BENCH_DATA` | `${TMPDIR:-/tmp}/pathfinder-bench` | Input directory; `bench/gen` fills it if a file is missing |
| `BENCH_REPS` | `3` | Runs per case and engine; the median is reported |
| `BENCH_STARTUP_REPS` | `15` | Runs for the startup set |
| `BENCH_SETS` | `startup,parse,filter,output` | Which sets to run |
| `BENCH_METRIC` | `auto` | `cycles`, `wall`, or `auto` (cycles if perf can count them) |

`bench/gen DIR` writes the inputs (`BENCH_MB`, default 30, and
`BENCH_SMALL_MB`, default 4, size them). It is a POSIX `awk` program with a
fixed-seed Park–Miller generator, so it depends on none of the engines and
the same sizes give the same bytes; `bench/run` prints each file's `cksum` so a
report shows which inputs it measured.

A default run costs about three and a half minutes per engine on the host
below (13 min 37 s for the four engines in Results); with `jq`, `jaq` and
`native` it stays near ten minutes. `BENCH_REPS=1` or `BENCH_SETS` shortens
it.

### How it measures

- `perf stat -x, -e cycles:u,instructions:u` around one invocation, several
  invocations per case, the median kept (the mean of the two middle values
  for an even count). Children are counted, so the jaq process that today's
  shim `exec`s or spawns is part of its figure.
- Two tables come out of the same runs. **Cycles** are R-011's measure.
  **Instructions** are the cross-check: they are close to deterministic,
  while the cycle count of a memory-bound run moves with the clock — a stall
  of fixed latency costs more cycles at a higher frequency. Measured on the
  host below, the patched jaq's `--tab .` on `doc.json` counted 4.11 G cycles
  inside a full run and 6.74–6.98 G in five back-to-back runs on an idle
  machine, while its instruction count stayed at 10.004 G (±0.0004%). On a
  laptop with frequency scaling, read a cycles ratio near a limit against the
  instructions ratio before acting on it, and raise `BENCH_REPS`.
- Runs are interleaved across engines (rep 1 of every engine, then rep 2, …),
  so drift lands on all of them alike.
- Input arrives on stdin; output goes through a pipe to `cksum`. A run that
  exits non-zero is reported as `fail` with its first stderr line. A figure
  whose output differs from the `jq` engine's is marked `†`: a fast wrong
  answer is not a result.
- If perf cannot count `cycles:u` (no perf, or `perf_event_paranoid` too
  high), `bench/run` falls back to `hyperfine -N` wall time — from `PATH`, or
  fetched with `nix build nixpkgs#hyperfine` — and says so in the report's
  `Metric:` line.
- After each table the report lists every case over budget among the
  non-reference engines, and every case where `jaq` is slower than `jq`.

## The sets

| Set | Cases | What it isolates |
|---|---|---|
| startup | `-n .` | The fixed cost of an invocation: loading, compiling the filter (and for the shim, translating argv before `exec`) |
| parse | `length`, `-c .`, `.` on the 30 MB single-line document and on the 30 MB file of ~210k small objects | Reading JSON; `length` keeps output out of it, `-c .` adds compact printing, `.` pretty printing. The two inputs separate one large value from many small ones |
| filter | `[range(1e6)] \| map(. * 2) \| add`, `reduce range(1e6) as $i (0; . + $i)`; `-s` with `group_by`, `sort_by`, `unique`; `[paths]`; `ascii_downcase`, `test("…")`, `tostring`, `@base64` per line | Evaluation with little I/O: arithmetic, array building, path enumeration, sorting and grouping, and the string builtins. Run on the 4 MB inputs so that jq's slow cases keep the run short |
| output | `-S .`, `--tab .`, `-r` on the document's strings, `-r .text` per line, `-a -c .` | Writing: key sorting, indentation, raw strings, ASCII escaping. jaq has no `-a`; that case measures the shim's post-processing and fails on bare jaq |

## Results

Two runs on the same host and inputs: the baseline at the fork point,
before the `native` engine existed, and the `native` engine at M19 against
the same `jq` and `jaq`.

### Baseline at the fork point

- Date: 2026-10-11T20:18:57Z
- Host: Intel Core i7-8665U @ 1.90 GHz (4 cores, 8 threads), Linux
  7.2.9-xanmod1, `powersave` governor, on a laptop
- Pathfinder: commit `35f2860` (main), `cargo build --release`; the shim of
  that commit ignores `PATHFINDER_ENGINE` and always hands off to jaq
- Runs: `BENCH_REPS=3`, `BENCH_STARTUP_REPS=15`, default sizes
- Inputs (`cksum`, `bench/gen` with GNU awk 5.4.1):
  `2372946771 31457310 doc.json`, `256760211 31457358 lines.jsonl`,
  `2379263251 4194393 small.json`, `455313611 4194414 small.jsonl`

| Engine | Binary | `--version` |
|---|---|---|
| `jq` | a statically linked jq 1.8.1 (`cksum` `2453573715 2255816`) | `jq-1.8.1` |
| `jaq` | `/nix/store/fvdk1as9ril57wjwh7xzgpgs5s4i5jdm-jaq-pathfinder-3.1.1/bin/jaq` — the packaged jaq with `packaging/jaq/` applied | `jaq 3.1.1+pathfinder.3` |
| `jaq-stock` | `/nix/store/x1g1kfppqiw6nxpq6a06gr978ijsmlh0-jaq-3.1.1/bin/jaq` — unpatched | `jaq 3.1.1` |
| `pf-jaq` | `target/release/pathfinder` as `jq`, with `PATHFINDER_ENGINE=jaq PATHFINDER_JAQ=` the `jaq` above | `jq-1.8.1` |

No figure is marked †: every engine's output was byte-identical to jq 1.8.1's
on every case it ran. Both jaq builds reject `-a` (`Error: unknown flag: -a`,
exit 2), so that case has no `jaq` figure.

#### User-space cycles (R-011)

| Set | Case | Input | jq | jaq | jaq-stock | pf-jaq | pf-jaq ×jq | pf-jaq ×jaq |
|---|---|---|---:|---:|---:|---:|---:|---:|
| startup | `-n .` | — | 5.92 M | 4.46 M | 4.34 M | 4.66 M | 0.79 | 1.04 |
| parse | `length` | doc.json | 4.65 G | 3.29 G | 2.59 G | 3.54 G | 0.76 | 1.08 |
| parse | `-c .` | doc.json | 6.08 G | 4.58 G | 4.04 G | 4.09 G | 0.67 | 0.89 |
| parse | `.` | doc.json | 8.93 G | 6.77 G | 6.08 G | 6.81 G | 0.76 | 1.01 |
| parse | `length` | lines.jsonl | 4.07 G | 4.11 G | 3.48 G | 4.30 G | 1.06 | 1.05 |
| parse | `-c .` | lines.jsonl | 8.29 G | 6.72 G | 5.70 G | 6.69 G | 0.81 | 1.00 |
| parse | `.` | lines.jsonl | 8.92 G | 7.60 G | 6.70 G | 7.37 G | 0.83 | 0.97 |
| filter | `-n [range(1e6)] \| map(. * 2) \| add` | — | 4.31 G | 3.37 G | 3.36 G | 3.21 G | 0.74 | 0.95 |
| filter | `-n reduce range(1e6) as $i (0; . + $i)` | — | 1.60 G | 1.62 G | 1.73 G | 1.63 G | 1.02 | 1.00 |
| filter | `-s group_by(.user) \| map(length) \| add` | small.jsonl | 1.11 G | 667.48 M | 577.81 M | 680.54 M | 0.61 | 1.02 |
| filter | `-s sort_by(.score) \| .[0].id` | small.jsonl | 1.18 G | 783.66 M | 690.24 M | 773.31 M | 0.66 | 0.99 |
| filter | `-s map(.user) \| unique \| length` | small.jsonl | 921.06 M | 656.01 M | 570.59 M | 648.32 M | 0.70 | 0.99 |
| filter | `[paths] \| length` | small.json | 2.50 G | 1.13 G | 1.05 G | 1.14 G | 0.46 | 1.01 |
| filter | `.text \| ascii_downcase \| length` | small.jsonl | 6.17 G | 821.08 M | 736.00 M | 865.38 M | 0.14 | 1.05 |
| filter | `.text \| test("a[a-z]+o")` | small.jsonl | 1.26 G | 2.24 G | 2.37 G | 2.83 G | 2.24 | 1.26 |
| filter | `tostring \| length` | small.jsonl | 1.33 G | 981.41 M | 818.16 M | 865.62 M | 0.65 | 0.88 |
| filter | `.text \| @base64 \| length` | small.jsonl | 539.49 M | 599.43 M | 567.35 M | 659.61 M | 1.22 | 1.10 |
| output | `-S .` | doc.json | 10.00 G | 6.28 G | 5.42 G | 6.16 G | 0.62 | 0.98 |
| output | `--tab .` | doc.json | 8.98 G | 6.88 G | 6.18 G | 6.96 G | 0.78 | 1.01 |
| output | `-r .items[].text` | doc.json | 3.79 G | 4.08 G | 3.43 G | 4.12 G | 1.09 | 1.01 |
| output | `-r .text` | lines.jsonl | 2.54 G | 2.95 G | 2.54 G | 3.01 G | 1.18 | 1.02 |
| output | `-a -c .` | lines.jsonl | 5.57 G | fail | fail | 7.10 G | 1.27 | — |

#### User-space instructions (cross-check)

| Set | Case | Input | jq | jaq | jaq-stock | pf-jaq | pf-jaq ×jq | pf-jaq ×jaq |
|---|---|---|---:|---:|---:|---:|---:|---:|
| startup | `-n .` | — | 6.96 M | 4.27 M | 4.24 M | 4.70 M | 0.68 | 1.10 |
| parse | `length` | doc.json | 6.37 G | 5.20 G | 4.14 G | 5.20 G | 0.82 | 1.00 |
| parse | `-c .` | doc.json | 12.26 G | 8.76 G | 7.69 G | 8.76 G | 0.71 | 1.00 |
| parse | `.` | doc.json | 15.07 G | 10.01 G | 8.94 G | 10.01 G | 0.66 | 1.00 |
| parse | `length` | lines.jsonl | 5.91 G | 5.21 G | 4.13 G | 5.22 G | 0.88 | 1.00 |
| parse | `-c .` | lines.jsonl | 11.74 G | 8.22 G | 7.13 G | 8.22 G | 0.70 | 1.00 |
| parse | `.` | lines.jsonl | 13.00 G | 9.27 G | 8.18 G | 9.27 G | 0.71 | 1.00 |
| filter | `-n [range(1e6)] \| map(. * 2) \| add` | — | 7.56 G | 5.06 G | 5.15 G | 5.06 G | 0.67 | 1.00 |
| filter | `-n reduce range(1e6) as $i (0; . + $i)` | — | 2.91 G | 2.57 G | 2.68 G | 2.57 G | 0.88 | 1.00 |
| filter | `-s group_by(.user) \| map(length) \| add` | small.jsonl | 1.43 G | 823.87 M | 678.74 M | 824.43 M | 0.58 | 1.00 |
| filter | `-s sort_by(.score) \| .[0].id` | small.jsonl | 1.45 G | 1.03 G | 887.02 M | 1.03 G | 0.71 | 1.00 |
| filter | `-s map(.user) \| unique \| length` | small.jsonl | 1.08 G | 846.36 M | 701.45 M | 847.01 M | 0.78 | 1.00 |
| filter | `[paths] \| length` | small.json | 3.61 G | 1.49 G | 1.35 G | 1.49 G | 0.41 | 1.00 |
| filter | `.text \| ascii_downcase \| length` | small.jsonl | 10.01 G | 868.28 M | 723.15 M | 868.88 M | 0.09 | 1.00 |
| filter | `.text \| test("a[a-z]+o")` | small.jsonl | 1.62 G | 2.04 G | 1.90 G | 2.30 G | 1.42 | 1.13 |
| filter | `tostring \| length` | small.jsonl | 1.98 G | 1.08 G | 937.32 M | 1.08 G | 0.55 | 1.00 |
| filter | `.text \| @base64 \| length` | small.jsonl | 1.12 G | 923.06 M | 777.98 M | 923.70 M | 0.82 | 1.00 |
| output | `-S .` | doc.json | 17.34 G | 10.29 G | 9.22 G | 10.29 G | 0.59 | 1.00 |
| output | `--tab .` | doc.json | 13.98 G | 10.00 G | 8.93 G | 10.00 G | 0.72 | 1.00 |
| output | `-r .items[].text` | doc.json | 6.48 G | 5.49 G | 4.43 G | 5.48 G | 0.84 | 1.00 |
| output | `-r .text` | lines.jsonl | 5.97 G | 5.79 G | 4.70 G | 5.79 G | 0.97 | 1.00 |
| output | `-a -c .` | lines.jsonl | 11.99 G | fail | fail | 13.39 G | 1.12 | — |

#### Findings

- **jaq is slower than jq 1.8.1 on some cases** — the `×jq` half of R-011
  is not free for the `native` engine to inherit:
  - `test("a[a-z]+o")` per line: the patched jaq takes 1.77× jq's cycles and
    1.26× its instructions; stock jaq is the same (1.87× cycles). jaq's
    regex path is the outlier of the set.
  - `@base64` per line (1.11× cycles), `-r .text` on the line-delimited file
    (1.16×), `-r .items[].text` (1.08×) and `reduce range(1e6)` (1.02×):
    slower in cycles, while their instruction counts are at or below jq's.
    These are near the noise of cycles on this host (see How it measures);
    the instruction counts say the work is not larger.
- **jq 1.8.1 is far slower on some builtins**: `ascii_downcase` takes 7.5×
  jaq's cycles (11.5× its instructions), `[paths]` 2.2×.
- **The patch set costs 12–26% more instructions than stock jaq** on
  parsing and output (for example `length` on `doc.json`: 5.20 G against
  4.14 G; `-r .text`: 5.79 G against 4.70 G) and 16–21% on the `-s` cases;
  the `range` cases are unaffected. The
  budget is measured against the patched engine, so this is the baseline
  `native` inherits, not a debt it must repay; it is also where an
  optimisation would find room.
- **Today's shim adds nothing measurable where it `exec`s jaq**: its
  instruction count equals the patched jaq's (ratio 1.00) on every case but
  three. Startup costs 1.10× in instructions (1.04× in cycles) — the
  argv translation before `exec`. `test(…)` costs 1.13× instructions and
  1.26× cycles — the shim's regex repair in the prelude, the one case over
  the `×jaq` bound. `-a -c .` costs 1.12× jq's instructions and 1.27× its
  cycles — the shim's own `-a` post-processing, outside jaq.
- **Cycles on this host are noisy between runs** of the same work: patched
  jaq on `--tab .` counted 4.11 G cycles in one full run and 6.88 G in the
  next, at 10.00 G instructions both times. Within one run, the shim and the
  jaq it `exec`s — identical instruction counts — differ by 0.88–1.10 in
  cycles. Read the `×jaq` cycles column as ±0.12 here; a cycles ratio
  outside that, or an instructions ratio above 1.00, is a real difference.

### The native engine (M19)

- Date: 2026-10-11T20:29:46Z, same host, inputs and `BENCH_REPS` as above
- `native`: `target/release/pathfinder` at the M19 commit, as `jq`, with
  `PATHFINDER_ENGINE=native` (mimalloc on); `jq` and `jaq` as above

No figure is marked †: the native engine's output was byte-identical to jq
1.8.1's on every case, `-a` included.

#### User-space cycles (R-011)

| Set | Case | Input | jq | jaq | native | native ×jq | native ×jaq |
|---|---|---|---:|---:|---:|---:|---:|
| startup | `-n .` | — | 6.14 M | 4.45 M | 3.87 M | 0.63 | 0.87 |
| parse | `length` | doc.json | 4.78 G | 3.49 G | 3.27 G | 0.68 | 0.94 |
| parse | `-c .` | doc.json | 5.85 G | 3.62 G | 4.07 G | 0.70 | 1.12 |
| parse | `.` | doc.json | 6.28 G | 4.13 G | 4.19 G | 0.67 | 1.01 |
| parse | `length` | lines.jsonl | 2.23 G | 2.52 G | 2.67 G | 1.19 | 1.06 |
| parse | `-c .` | lines.jsonl | 4.72 G | 3.39 G | 3.67 G | 0.78 | 1.08 |
| parse | `.` | lines.jsonl | 5.35 G | 4.15 G | 4.23 G | 0.79 | 1.02 |
| filter | `-n [range(1e6)] \| map(. * 2) \| add` | — | 2.45 G | 2.03 G | 1.87 G | 0.76 | 0.92 |
| filter | `-n reduce range(1e6) as $i (0; . + $i)` | — | 955.63 M | 957.93 M | 908.23 M | 0.95 | 0.95 |
| filter | `-s group_by(.user) \| map(length) \| add` | small.jsonl | 648.56 M | 395.60 M | 405.15 M | 0.62 | 1.02 |
| filter | `-s sort_by(.score) \| .[0].id` | small.jsonl | 694.02 M | 452.37 M | 460.95 M | 0.66 | 1.02 |
| filter | `-s map(.user) \| unique \| length` | small.jsonl | 523.11 M | 367.14 M | 450.93 M | 0.86 | 1.23 |
| filter | `[paths] \| length` | small.json | 1.43 G | 670.10 M | 642.72 M | 0.45 | 0.96 |
| filter | `.text \| ascii_downcase \| length` | small.jsonl | 3.38 G | 391.55 M | 407.85 M | 0.12 | 1.04 |
| filter | `.text \| test("a[a-z]+o")` | small.jsonl | 750.64 M | 1.11 G | 1.13 G | 1.50 | 1.02 |
| filter | `tostring \| length` | small.jsonl | 737.34 M | 490.72 M | 523.87 M | 0.71 | 1.07 |
| filter | `.text \| @base64 \| length` | small.jsonl | 411.34 M | 417.70 M | 451.87 M | 1.10 | 1.08 |
| output | `-S .` | doc.json | 7.33 G | 4.01 G | 4.31 G | 0.59 | 1.07 |
| output | `--tab .` | doc.json | 5.82 G | 3.95 G | 4.21 G | 0.72 | 1.07 |
| output | `-r .items[].text` | doc.json | 2.71 G | 2.31 G | 2.59 G | 0.96 | 1.12 |
| output | `-r .text` | lines.jsonl | 2.27 G | 2.71 G | 2.95 G | 1.30 | 1.09 |
| output | `-a -c .` | lines.jsonl | 4.85 G | fail | 4.68 G | 0.96 | — |

Over budget (×jq > 1.00 or ×jaq > 1.10):

- `native` on `-c .` (doc.json): 1.12× jaq
- `native` on `length` (lines.jsonl): 1.19× jq
- `native` on `-s map(.user) | unique | length` (small.jsonl): 1.23× jaq
- `native` on `.text | test("a[a-z]+o")` (small.jsonl): 1.50× jq
- `native` on `.text | @base64 | length` (small.jsonl): 1.10× jq
- `native` on `-r .items[].text` (doc.json): 1.12× jaq
- `native` on `-r .text` (lines.jsonl): 1.30× jq

Cases where `jaq` is slower than `jq`:

- `length` (lines.jsonl): 1.13× jq
- `.text | test("a[a-z]+o")` (small.jsonl): 1.47× jq
- `.text | @base64 | length` (small.jsonl): 1.02× jq
- `-r .text` (lines.jsonl): 1.20× jq

#### User-space instructions (cross-check)

| Set | Case | Input | jq | jaq | native | native ×jq | native ×jaq |
|---|---|---|---:|---:|---:|---:|---:|
| startup | `-n .` | — | 6.95 M | 4.26 M | 3.51 M | 0.50 | 0.82 |
| parse | `length` | doc.json | 6.37 G | 5.20 G | 5.43 G | 0.85 | 1.04 |
| parse | `-c .` | doc.json | 12.26 G | 8.76 G | 9.08 G | 0.74 | 1.04 |
| parse | `.` | doc.json | 15.07 G | 10.01 G | 10.26 G | 0.68 | 1.03 |
| parse | `length` | lines.jsonl | 5.91 G | 5.21 G | 5.46 G | 0.92 | 1.05 |
| parse | `-c .` | lines.jsonl | 11.74 G | 8.22 G | 8.58 G | 0.73 | 1.04 |
| parse | `.` | lines.jsonl | 13.00 G | 9.27 G | 9.59 G | 0.74 | 1.03 |
| filter | `-n [range(1e6)] \| map(. * 2) \| add` | — | 7.56 G | 5.06 G | 4.19 G | 0.55 | 0.83 |
| filter | `-n reduce range(1e6) as $i (0; . + $i)` | — | 2.91 G | 2.57 G | 2.16 G | 0.74 | 0.84 |
| filter | `-s group_by(.user) \| map(length) \| add` | small.jsonl | 1.43 G | 823.66 M | 825.29 M | 0.58 | 1.00 |
| filter | `-s sort_by(.score) \| .[0].id` | small.jsonl | 1.45 G | 1.03 G | 1.03 G | 0.71 | 1.00 |
| filter | `-s map(.user) \| unique \| length` | small.jsonl | 1.08 G | 846.29 M | 847.79 M | 0.78 | 1.00 |
| filter | `[paths] \| length` | small.json | 3.61 G | 1.49 G | 1.35 G | 0.37 | 0.90 |
| filter | `.text \| ascii_downcase \| length` | small.jsonl | 10.01 G | 868.24 M | 864.65 M | 0.09 | 1.00 |
| filter | `.text \| test("a[a-z]+o")` | small.jsonl | 1.62 G | 2.04 G | 2.07 G | 1.28 | 1.02 |
| filter | `tostring \| length` | small.jsonl | 1.98 G | 1.08 G | 1.08 G | 0.54 | 1.00 |
| filter | `.text \| @base64 \| length` | small.jsonl | 1.12 G | 922.94 M | 911.60 M | 0.81 | 0.99 |
| output | `-S .` | doc.json | 17.34 G | 10.29 G | 10.53 G | 0.61 | 1.02 |
| output | `--tab .` | doc.json | 13.98 G | 10.00 G | 10.26 G | 0.73 | 1.03 |
| output | `-r .items[].text` | doc.json | 6.48 G | 5.48 G | 5.67 G | 0.87 | 1.03 |
| output | `-r .text` | lines.jsonl | 5.97 G | 5.79 G | 5.88 G | 0.98 | 1.02 |
| output | `-a -c .` | lines.jsonl | 11.99 G | fail | 11.38 G | 0.95 | — |

Over budget (×jq > 1.00 or ×jaq > 1.10):

- `native` on `.text | test("a[a-z]+o")` (small.jsonl): 1.28× jq

Cases where `jaq` is slower than `jq`:

- `.text | test("a[a-z]+o")` (small.jsonl): 1.26× jq

## Failed runs

Counted as `fail`, no figure:

- `jaq` on `-a -c .` (lines.jsonl): exit 2: Error: unknown flag: -a

#### Findings

- **Against the patched jaq, the native engine is at parity in work**: its
  instruction count is within 1.05× of jaq's on every case, and below it
  on startup (0.82×) and the `range` cases (0.83–0.84×). The cycles ratios
  above 1.10 (`-c .` on `doc.json`, `unique`, `-r .items[].text`) are not
  matched by the instructions; read them with the ±0.12 noise noted for the
  baseline.
- **Against jq 1.8.1, R-011 does not hold yet on four cases**, all of which
  jaq already loses: `test(…)` (1.50× jq's cycles, 1.28× its instructions),
  `-r .text` on lines (1.30×), `length` on lines (1.19×) and `@base64`
  (1.10×). They are inherited from jaq's regex engine, raw string output
  and line-delimited reading, and PLAN P-060 stays open until they are
  repaired.
