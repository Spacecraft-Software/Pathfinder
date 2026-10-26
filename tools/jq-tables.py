#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Write jq 1.8.1's parser tables as Rust (crates/pathfinder/src/syntax/jq_tables.rs).

    tools/jq-tables.py path/to/jq-1.8.1/src > crates/pathfinder/src/syntax/jq_tables.rs
    rustfmt --edition 2024 crates/pathfinder/src/syntax/jq_tables.rs

`src/parser.c` is the one jq's release tarball ships (bison 3.8.2,
`parse.error verbose`): the tables the jq 1.8.1 binary runs. `src/builtin.c`,
`src/builtin.h` and `src/libm.h` give the builtins jq implements in C, whose arguments jq
compiles inline rather than as closures. Get the tarball with

    curl -fsSL -O https://github.com/jqlang/jq/releases/download/jq-1.8.1/jq-1.8.1.tar.gz
    echo "2be64e7129cecb11d5906290eba10af694fb9e3e7f9fc208a311dc33ca837eb0  jq-1.8.1.tar.gz" | sha256sum -c -

The script reads the tables and constants only; nothing of jq's C runs.
"""

import re
import sys

ARRAYS = [
    ("yytranslate", "u8"),
    ("yypact", "i16"),
    ("yydefact", "u8"),
    ("yypgoto", "i16"),
    ("yydefgoto", "u8"),
    ("yytable", "i16"),
    ("yycheck", "i16"),
    ("yyr1", "u8"),
    ("yyr2", "u8"),
]
CONSTS = ["YYFINAL", "YYLAST", "YYNTOKENS", "YYNRULES", "YYNSTATES", "YYMAXUTOK"]
NEG_CONSTS = ["YYPACT_NINF", "YYTABLE_NINF"]


def array(c, name):
    m = re.search(r"static const \w+ " + name + r"\[\] =\s*\{(.*?)\};", c, re.S)
    if not m:
        sys.exit(f"no {name} in parser.c")
    return [int(x) for x in re.findall(r"-?\d+", m.group(1))]


def const(c, name):
    m = re.search(r"#define " + name + r"\s+\(?(-?\d+)\)?", c)
    if not m:
        sys.exit(f"no {name} in parser.c")
    return int(m.group(1))


def names(c):
    m = re.search(r"static const char \*const yytname\[\] =\s*\{(.*?)YY_NULLPTR", c, re.S)
    if not m:
        sys.exit("no yytname in parser.c")
    out = []
    for lit in re.findall(r'"((?:[^"\\]|\\.)*)"', m.group(1)):
        out.append(lit.encode().decode("unicode_escape"))
    return out


def tokens(c):
    m = re.search(r"enum yytokentype\s*\{(.*?)\};", c, re.S)
    return [(n, int(v)) for n, v in re.findall(r"(\w+) = (-?\d+)", m.group(1))]


def c_builtins(src):
    """(name, arguments) of every C builtin: `function_list` in builtin.c,
    with its BINOP and libm macros expanded. The count CFUNC takes includes
    the input, which is no argument."""
    b = open(src + "/builtin.c", encoding="utf-8").read()
    libm = open(src + "/libm.h", encoding="utf-8").read()
    body = re.search(r"function_list\[\] = \{(.*?)\n\};", b, re.S).group(1)
    out = set()
    for name, n in re.findall(r'CFUNC\(f_\w+, "([^"]+)", (\d+)\)', body):
        out.add((name, int(n) - 1))
    for name, n in re.findall(r'\{f_\w+, "([^"]+)", (\d+)\}', body):
        out.add((name, int(n) - 1))
    h = open(src + "/builtin.h", encoding="utf-8").read()
    binops = re.search(r"#define BINOPS \\\n(.*?)\n\n", h, re.S)
    for op in re.findall(r"BINOP\((\w+)\)", binops.group(1)):
        out.add(("_" + op, 2))
    args = {"DD": 0, "DA": 0, "DDD": 2, "DDDD": 3}
    for kind, name in re.findall(r"^LIBM_(DD|DA|DDD|DDDD)(?:_NO)?\((\w+)", libm, re.M):
        out.add((name, args[kind]))
    return sorted(out)


def rust_str(s):
    return '"' + s.replace("\\", "\\\\").replace('"', '\\"') + '"'


def main():
    src = sys.argv[1]
    c = open(src + "/parser.c", encoding="utf-8").read()
    if 'YYBISON_VERSION "3.8.2"' not in c:
        sys.exit("expected the parser.c of jq 1.8.1's tarball (bison 3.8.2)")
    o = []
    w = o.append
    # REUSE-IgnoreStart
    w("// SPDX-FileCopyrightText: 2012 Stephen Dolan and jq contributors")
    w("// SPDX-License-Identifier: MIT")
    # REUSE-IgnoreEnd
    w("")
    w("//! jq 1.8.1's parser tables, as bison 3.8.2 generated them into the")
    w("//! `src/parser.c` of jq's release tarball. Written by `tools/jq-tables.py`;")
    w("//! regenerate rather than edit.")
    w("#![allow(")
    w("    dead_code,")
    w("    non_upper_case_globals,")
    w("    reason = \"generated: bison's full tables, named as bison names them\"")
    w(")]")
    w("")
    for n in CONSTS:
        w(f"pub const {n}: i32 = {const(c, n)};")
    for n in NEG_CONSTS:
        w(f"pub const {n}: i32 = {const(c, n)};")
    w("")
    w("/// jq's token codes (`enum yytokentype`).")
    w("pub mod tok {")
    for n, v in tokens(c):
        if v >= 0:
            w(f"    pub const {n}: i32 = {v};")
    w("}")
    w("")
    for n, t in ARRAYS:
        vals = array(c, n)
        w(f"pub static {n.upper()}: [{t}; {len(vals)}] = [")
        for i in range(0, len(vals), 16):
            w("    " + ", ".join(str(v) for v in vals[i : i + 16]) + ",")
        w("];")
        w("")
    nm = names(c)
    w(f"/// Symbol names (`yytname`), as bison spells them in its messages.")
    w(f"pub static YYTNAME: [&str; {len(nm)}] = [")
    for s in nm:
        w(f"    {rust_str(s)},")
    w("];")
    w("")
    cb = c_builtins(src)
    w("/// jq's C builtins and their arguments (the input not counted).")
    w(f"pub static C_BUILTINS: [(&str, usize); {len(cb)}] = [")
    for name, n in cb:
        w(f"    ({rust_str(name)}, {n}),")
    w("];")
    sys.stdout.write("\n".join(o) + "\n")


main()
