#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Random regexes through jq 1.8.1 and the native engine, compared.

    tools/regex-diff.py JQ SHIM [--seed N] [--cases N] [--kind syntax|functions]

`syntax` draws patterns from every construct `ONIG_SYNTAX_PERL_NG` has
(classes, groups, lookaround, quantifiers, backreferences, escapes,
options, `\\K`, `\\R`, `\\X`) under jq's flags, through `match`;
`functions` stresses captures, case folding, intervals and malformed
patterns through every jq regex function. Each batch runs in one process
per side; a difference prints the case and both answers. A pattern that
makes jq itself loop (a `\\K` in a lookbehind can) times the batch out:
`--dump` writes the cases, to time them one by one. This found every quirk M22 reproduces.
"""

import argparse
import json
import os
import random
import subprocess
import sys

SYNTAX_ATOMS = [
    "a", "b", "c", "é", "ß", "K", "ſ", "İ", "i", "I", "s", "S", "x", " ", "1", "٣", "😀",
    "\\n", "\\t", ".", "\\w", "\\W", "\\d", "\\D", "\\s", "\\S", "\\h", "[a-c]", "[^a]",
    "[[:alpha:]]", "[\\w-]", "[ß]", "[^\\x00-\\xff]", "\\p{L}", "\\p{^L}", "\\P{Greek}",
    "[[:^digit:]]", "[a-é]", "[-a]", "[a-]", "[]a]", "\\x41", "\\x{e9}", "\\101", "\\Qa.b\\E",
    "(?#c)", "\\K", "\\R", "\\X", "\\N", "\\O", "\\b", "\\B", "^", "$", "\\A", "\\z", "\\Z",
    "\\G", "\\e", "\\cA", "a{,2}", "\\u0041", "\\k<n>", "\\1", "\\2", "",
]
SYNTAX_QUANTS = ["*", "+", "?", "{2}", "{1,3}", "{2,}", "*?", "+?", "??", "*+", "++", "?+",
                 "{1,2}?", "{0}", "{0,1}"]
SYNTAX_GROUPS = ["(", "(?:", "(?<n>", "(?=", "(?!", "(?<=", "(?<!", "(?>", "(?i:", "(?i)",
                 "(?m:", "(?s:", "(?x:", "(?-i:"]
SYNTAX_TEXT = ["a", "b", "c", "é", "ß", "ss", "SS", "K", "k", "ſ", "İ", "i̇", "i", "I", " ",
               "\n", "1", "٣", "😀", "x", "ab", "\r\n", "é", "👍🏽"]

FN_LITERALS = ["a", "b", "s", "S", "ss", "SS", "ß", "ẞ", "ſ", "ﬃ", "ffi", "FFI", "İ", "i",
               "I", "ı", "i̇", "K", "k", "K", "é", "É", "é", "Σ", "σ", "ς", "ǅ", "ǆ",
               "Ǆ", "😀", " ", "-", "\\.", "\\(", "x", "1", "٣"]
FN_CLASSES = ["[a-z]", "[A-Z]", "[^a-z]", "[[:alpha:]]", "[[:^alpha:]]", "[[:word:]]",
              "[[:punct:]]", "[[:upper:]]", "[[:lower:]]", "[[:space:]]", "[[:xdigit:]]",
              "[[:ascii:]]", "[[:foo:]]", "[[:alpha]", "[a-\\w]", "[\\w-z]", "[a-c-e]",
              "[z-a]", "[\\x{61}-\\x{7a}]", "[\\x{61 62}]", "[\\x{61-63}]", "\\p{Lu}",
              "\\p{Ll}", "\\p{Greek}", "\\p{greek}", "\\p{Is_Greek}",
              "\\p{In_Greek_and_Coptic}", "\\p{Han}", "\\p{^Lu}", "\\p{L&}", "\\p{Any}",
              "\\p{Assigned}", "\\p{Emoji}", "\\p{Word}", "\\p{Alnum}", "\\p{nosuch}",
              "[ßs]", "[ẞ]", "[^ß]", "[ﬃ]", "[İ]", "[σ]"]
FN_ESCAPES = ["\\x{61 62 63}", "\\x{61 }", "\\x{ 61}", "\\x{110000}", "\\o{141 142}", "\\0",
              "\\00", "\\012", "\\x", "\\x4", "\\Qa*b", "\\Qa\\E*", "\\E", "\\cA", "\\c?",
              "\\a", "\\f", "\\v", "\\y", "\\Y", "\\X", "\\R"]
FN_QUANTS = ["*", "+", "?", "{2}", "{1,3}", "{2,}", "{,2}", "{,}", "{x}", "{2,1}", "{1,2}+",
             "{2}?", "{1,}?", "*?", "+?", "??", "*+", "++", "?+", "{3,3}", "{0,0}",
             "{100001}"]
FN_TEXT = ["a", "b", "s", "S", "ß", "ẞ", "ſ", "ss", "SS", "sS", "ﬃ", "ffi", "FFI", "İ", "i",
           "I", "ı", "i̇", "K", "k", "K", "é", "É", "é", "Σ", "σ", "ς", "ǅ", "ǆ", "😀",
           "  ", "-", ".", "(", "x", "1", "٣", "\n", "ab", "aab", "ba"]
FN_FLAGS = ["", "g", "gi", "gx", "i", "x", "gl", "gn", "gix", "gm"]
FUNCTIONS = ["match", "test", "capture", "scan", "splits", "sub", "gsub"]


def syntax_case(r):
    def expr(d):
        if d > 3 or r.random() < 0.3:
            return r.choice(SYNTAX_ATOMS)
        k = r.random()
        if k < 0.25:
            return "".join(expr(d + 1) for _ in range(r.randint(2, 3)))
        if k < 0.4:
            return "|".join(expr(d + 1) for _ in range(r.randint(2, 3)))
        if k < 0.75:
            return r.choice(SYNTAX_GROUPS) + expr(d + 1) + ")"
        if r.random() < 0.5:
            return "(?:" + expr(d + 1) + ")" + r.choice(SYNTAX_QUANTS)
        return expr(d + 1) + r.choice(SYNTAX_QUANTS)

    s = "".join(r.choice(SYNTAX_TEXT) for _ in range(r.randint(0, 6)))
    flags = r.choice(["", "g", "gi", "gx", "gm", "gl", "gn", "gs", "gp", "i", "x"])
    return {"s": s, "r": expr(0), "f": flags, "fn": "match"}


def functions_case(r):
    def atom(d):
        k = r.random()
        if k < 0.35:
            return r.choice(FN_LITERALS)
        if k < 0.55:
            return r.choice(FN_CLASSES)
        if k < 0.62:
            return r.choice(FN_ESCAPES)
        if d > 2:
            return r.choice(FN_LITERALS)
        if k < 0.75:
            return "(" + seq(d + 1) + ")"
        if k < 0.82:
            return "(?<" + r.choice(["a", "b", "a", "n1"]) + ">" + seq(d + 1) + ")"
        if k < 0.86:
            return r.choice(["\\1", "\\2", "\\k<a>", "\\k<b>", "\\k<1>", "\\k<-1>",
                             "\\k<a+0>", "(?(1)x|y)", "(?(<a>)x)", "(?(a)x|y)"])
        if k < 0.92:
            return r.choice(["(?i)", "(?-i)", "(?x)", "(?m)", "(?s)", "(?i:", "(?ix:",
                             "(?=", "(?!", "(?<=", "(?<!", "(?>"]) + seq(d + 1) + ")"
        return "(?:" + seq(d + 1) + "|" + seq(d + 1) + ")"

    def seq(d):
        out = ""
        for _ in range(r.randint(1, 3)):
            a = atom(d)
            if r.random() < 0.35:
                a += r.choice(FN_QUANTS)
            out += a
        if r.random() < 0.05:
            out += r.choice([")", "(", "[", "]", "{", "\\", "(?", "(?<", "(?P<x>", "(?#"])
        return out

    s = "".join(r.choice(FN_TEXT) for _ in range(r.randint(0, 8)))
    return {"s": s, "r": seq(0), "f": r.choice(FN_FLAGS), "fn": r.choice(FUNCTIONS)}


PROGRAM = (
    '. as $c | try ($c.s | if $c.fn == "match" then [match($c.r; $c.f)]'
    ' elif $c.fn == "test" then test($c.r; $c.f)'
    ' elif $c.fn == "capture" then [capture($c.r; $c.f)]'
    ' elif $c.fn == "scan" then [scan($c.r; $c.f)]'
    ' elif $c.fn == "splits" then [splits($c.r; $c.f)]'
    ' elif $c.fn == "sub" then [sub($c.r; "<\\(.)>"; $c.f)]'
    ' else [gsub($c.r; "<\\(.)>"; $c.f)] end) catch ("ERR: " + .)'
)


def run(cmd, cases, env=None):
    p = subprocess.run(cmd + ["-c", PROGRAM], input=cases, capture_output=True,
                       text=True, timeout=300, env=env)
    return p.stdout.splitlines()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("jq")
    ap.add_argument("shim")
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--cases", type=int, default=3000)
    ap.add_argument("--kind", choices=["syntax", "functions"], default="syntax")
    ap.add_argument("--dump", help="write the cases here, one JSON per line, and stop")
    a = ap.parse_args()
    r = random.Random(a.seed)
    make = syntax_case if a.kind == "syntax" else functions_case
    cases = [make(r) for _ in range(a.cases)]
    text = "".join(json.dumps(c, ensure_ascii=False) + "\n" for c in cases)
    if a.dump:
        with open(a.dump, "w", encoding="utf-8") as f:
            f.write(text)
        return
    env = dict(os.environ, PATHFINDER_ENGINE="native")
    want, got = run([a.jq], text), run([a.shim], text, env)
    if len(want) != len(cases) or len(got) != len(cases):
        sys.exit(f"a side stopped early ({len(want)}, {len(got)} of {len(cases)}): "
                 "a case loops; bisect and drop it")
    bad = 0
    for c, w, g in zip(cases, want, got):
        if w != g:
            bad += 1
            print(json.dumps(c, ensure_ascii=False), "\n  jq  ", w, "\n  shim", g)
    print(f"{bad} of {len(cases)} differ")
    sys.exit(1 if bad else 0)


main()
