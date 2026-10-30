#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Random time builtin calls through jq 1.8.1 and the native engine, compared.

    tools/time-diff.py JQ SHIM [--seed N] [--cases N] [--tz ZONE]...

Each case is a time (seconds, fractional or negative, near the epoch, in
the far past or future; or a broken-down time array with fields out of
range, missing or not numbers) and one of `gmtime`, `localtime`,
`mktime`, `todate`, `fromdate`, `strftime`, `strflocaltime` and
`strptime`, with formats drawn from every conversion glibc has, its flags
and widths. `strptime` reads what jq's own `strftime` wrote, under the
same or another format, and text that is not a date. Every zone runs the
batch in one process per side (`TZ`); a difference prints the case and
both answers (M23).
"""

import argparse
import json
import os
import random
import subprocess
import sys

CONVERSIONS = list("aAbBcCdDeFgGhHIjklmMnpPrRsStTuUVwWxXyYzZ%")
FLAGS = ["", "", "", "-", "_", "0", "^", "#", "+"]
TEXT = ["", " ", "T", ":", "-", "/", "x", "Z", ", ", "  ", "\t"]

PROGRAM = (
    '. as $c | try ($c.v | if $c.fn == "gmtime" then gmtime'
    ' elif $c.fn == "localtime" then localtime'
    ' elif $c.fn == "mktime" then mktime'
    ' elif $c.fn == "todate" then todate'
    ' elif $c.fn == "fromdate" then fromdate'
    ' elif $c.fn == "strftime" then strftime($c.f)'
    ' elif $c.fn == "strflocaltime" then strflocaltime($c.f)'
    ' elif $c.fn == "roundtrip" then strftime($c.f) | [., (try strptime($c.g) catch ("ERR: " + .))]'
    ' else strptime($c.f) end) catch ("ERR: " + .)'
)


def seconds(r):
    k = r.random()
    if k < 0.3:
        return r.randint(-10**9, 2 * 10**9)
    if k < 0.45:
        return round(r.uniform(-10**9, 2 * 10**9), r.randint(1, 4))
    if k < 0.6:
        return r.randint(-86400 * 3, 86400 * 3)
    if k < 0.7:
        return r.choice([0, -1, 1, 951782400, 1709251199, 4102444800, 253402300799,
                         253402300800, -62135596800, -62135596801, 1e15, -1e15, 1e17])
    if k < 0.8:
        # Around the zones' daylight saving changes.
        return r.choice([1710054000, 1730613600, 1711846800, 1729990800]) + r.randint(-7200, 7200)
    return r.randint(-10**12, 10**12)


def field(r, lo, hi):
    k = r.random()
    if k < 0.7:
        return r.randint(lo, hi)
    if k < 0.8:
        return r.randint(lo - 100, hi + 100)
    if k < 0.9:
        return round(r.uniform(lo, hi), 2)
    return r.choice([-1, 0, 1, 1e10, -1e10, None, "1"])


def broken_down(r):
    a = [field(r, 1900, 2100), field(r, 0, 11), field(r, 1, 31), field(r, 0, 23),
         field(r, 0, 59), field(r, 0, 60), field(r, 0, 6), field(r, 0, 365)]
    if r.random() < 0.2:
        a = a[:r.randint(0, 7)]
    if r.random() < 0.05:
        a.append(1)
    return a


def fmt(r, n=None):
    out = ""
    for _ in range(n or r.randint(1, 5)):
        out += r.choice(TEXT)
        out += "%" + r.choice(FLAGS)
        if r.random() < 0.15:
            out += str(r.randint(0, 12))
        if r.random() < 0.05:
            out += r.choice("EO")
        out += r.choice(CONVERSIONS)
    if r.random() < 0.05:
        out += "%"
    return out


def case(r):
    fn = r.choice(["gmtime", "localtime", "mktime", "todate", "fromdate", "strftime",
                   "strflocaltime", "roundtrip", "roundtrip", "strptime"])
    if fn in ("gmtime", "localtime", "todate"):
        v = seconds(r) if r.random() < 0.9 else broken_down(r)
        if fn == "localtime" and isinstance(v, (int, float)) and abs(v) > 1.8e14:
            # Past about year 5 881 600 glibc's daylight saving rule
            # overflows an `int` (the day count in `compute_change`); the
            # shim keeps applying the rule.
            v = v % 1.8e14
        return {"fn": fn, "v": v}
    if fn == "mktime":
        return {"fn": fn, "v": broken_down(r) if r.random() < 0.9 else seconds(r)}
    if fn == "fromdate":
        y, mo, d = r.randint(1, 9999), r.randint(0, 13), r.randint(0, 32)
        h, mi, s = r.randint(0, 25), r.randint(0, 61), r.randint(0, 62)
        return {"fn": fn, "v": f"{y:04d}-{mo:02d}-{d:02d}T{h:02d}:{mi:02d}:{s:02d}Z"
                + r.choice(["", "", " ", "x", ".5"])}
    if fn in ("strftime", "strflocaltime"):
        v = seconds(r) if r.random() < 0.6 else broken_down(r)
        if fn == "strflocaltime" and isinstance(v, (int, float)) and abs(v) > 6e16:
            # jq aborts (an assertion in `jv_array_get`) when `localtime`
            # fails here; the shim raises `localtime`'s error instead.
            v = 0
        if fn == "strflocaltime" and isinstance(v, (int, float)) and abs(v) > 1.8e14:
            v = v % 1.8e14  # past glibc's daylight saving overflow, as below
        if fn == "strflocaltime" and isinstance(v, list):
            # Fields that normalise past the year of glibc's overflow.
            for i, bound in [(0, 5e6), (1, 6e7), (2, 2e9)]:
                if len(v) > i and isinstance(v[i], (int, float)) and abs(v[i]) > bound:
                    v[i] = 1
        return {"fn": fn, "v": v, "f": fmt(r)}
    if fn == "roundtrip":
        f = fmt(r, r.randint(1, 4))
        # `%s` reads back through `localtime`: within glibc's overflow.
        v = seconds(r) % 1.8e14
        return {"fn": fn, "v": v, "f": f, "g": f if r.random() < 0.7 else fmt(r)}
    words = ["2015-03-05T23:51:47Z", "10:15", "Thursday, March 05", "Mon", "Feb 29 2016",
             "1/2/03", "12 PM", "1425599621", "+0530", "-07:00", "Z", "53", "366", "x", ""]
    return {"fn": fn, "v": r.choice(words) + r.choice(["", " ", " trailing", "x"]),
            "f": fmt(r)}


def run(cmd, cases, env):
    p = subprocess.run(cmd + ["-c", PROGRAM], input=cases, capture_output=True,
                       text=True, timeout=300, env=env)
    return p.stdout.splitlines()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("jq")
    ap.add_argument("shim")
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--cases", type=int, default=3000)
    ap.add_argument("--tz", action="append")
    a = ap.parse_args()
    r = random.Random(a.seed)
    cases = [case(r) for _ in range(a.cases)]
    text = "".join(json.dumps(c, ensure_ascii=False) + "\n" for c in cases)
    bad = 0
    for tz in a.tz or ["UTC", "Asia/Kolkata", "America/New_York", "Europe/Berlin"]:
        env = dict(os.environ, TZ=tz, PATHFINDER_ENGINE="native")
        want, got = run([a.jq], text, env), run([a.shim], text, env)
        if len(want) != len(cases) or len(got) != len(cases):
            sys.exit(f"[{tz}] a side stopped early ({len(want)}, {len(got)} of {len(cases)})")
        for c, w, g in zip(cases, want, got):
            if w != g:
                bad += 1
                print(f"[{tz}]", json.dumps(c, ensure_ascii=False), "\n  jq  ", w, "\n  shim", g)
    print(f"{bad} differ")
    sys.exit(1 if bad else 0)


main()
