#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
# SPDX-License-Identifier: GPL-3.0-or-later
"""Write Oniguruma 6.9.10's Unicode tables as Rust (crates/pathfinder-regex/src/tables.rs).

    tools/onig-tables.py path/to/jq-1.8.1/vendor/oniguruma/src > crates/pathfinder-regex/src/tables.rs
    rustfmt --edition 2024 crates/pathfinder-regex/src/tables.rs

jq 1.8.1's release binary is built `--with-oniguruma=builtin`: these are the
tables it matches with. Get the tarball as `tools/jq-tables.py` says. Read:

- `unicode_property_data.c`: every property's code ranges (`CodeRanges[]`,
  indexed by ctype) and the property names (gperf's pool and wordlist);
- `unicode_fold_data.c`: case folding to one, two and three code points.
  The entries under "LOCALE" are the non-Turkish folds of `I` and `İ`
  (`USE_UNICODE_CASE_FOLD_TURKISH_AZERI` is off), so they are kept;
- `unicode_egcb_data.c`: grapheme cluster break classes, for `\\X`;
- `unicode.c`: the character types of the codes below 256, which
  Oniguruma answers from a table of its own, not from the ranges
  (`EncUNICODE_ISO_8859_1_CtypeTable`).

The script reads the tables only; nothing of Oniguruma's C runs.
"""

import re
import sys


def read(src, name):
    return open(f"{src}/{name}", encoding="utf-8").read()


def properties(c):
    """(ranges by CR name, ctype order of CR names, [(name, ctype)])."""
    ranges = {}
    for name, body in re.findall(
        r"static const OnigCodePoint\s+(CR_\w+)\[\] = \{ \d+,\n(.*?)\}; /\* END", c, re.S
    ):
        vals = [int(x, 16) for x in re.findall(r"0x([0-9a-f]+)", body)]
        ranges[name] = list(zip(vals[0::2], vals[1::2]))
    # Aliases: `#define CR_Alphabetic CR_Alpha`.
    alias = dict(re.findall(r"^#define (CR_\w+) (CR_\w+)$", c, re.M))
    order = re.search(r"const CodeRanges\[\] = \{(.*?)\};", c, re.S).group(1)
    order = [alias.get(n, n) for n in re.findall(r"CR_\w+", order)]
    pool = {
        int(i): s
        for i, s in re.findall(
            r'char unicode_prop_name_pool_str(\d+)\[sizeof\("([^"]*)"\)\];', c
        )
    }
    names = []
    for i, ctype in re.findall(r"\{pool_offset\((\d+)\),\s*(\d+)\}", c):
        names.append((pool[int(i)].lower(), int(ctype)))
    return ranges, order, sorted(set(names))


def folds(c, n):
    """[(folded sequence, [code points that fold to it])], Folds<n>."""
    body = re.search(
        rf"OnigUnicodeFolds{n}\[\] = \{{(.*?)#define FOLDS{n}_END_INDEX", c, re.S
    ).group(1)
    out = []
    for line in body.splitlines():
        line = re.sub(r"/\*.*?\*/", "", line).strip().rstrip(",")
        if not line or line.startswith("#define"):
            continue
        vals = [int(x, 0) for x in line.split(",")]
        key, count, codes = vals[:n], vals[n], vals[n + 1 :]
        assert len(codes) == count, line
        out.append((key, codes))
    return out


def egcb(c):
    classes = re.search(r"/\*\n(.*?)\*/\n\nstatic int EGCB_RANGE_NUM", c, re.S).group(1).split()
    rows = re.findall(r"\{0x([0-9a-f]+), 0x([0-9a-f]+), EGCB_(\w+) \}", c)
    return classes, [(int(a, 16), int(b, 16), k) for a, b, k in rows]


def ranges_lit(rs):
    return "[" + ", ".join(f"(0x{a:x}, 0x{b:x})" for a, b in rs) + "]"


def main():
    src = sys.argv[1]
    if "#define ONIGURUMA_VERSION_TEENY   10" not in read(src, "oniguruma.h"):
        sys.exit("expected the Oniguruma 6.9.10 vendored in jq 1.8.1's tarball")
    o = []
    w = o.append
    # REUSE-IgnoreStart
    w("// SPDX-FileCopyrightText: 2016-2024 K.Kosako")
    w("// SPDX-License-Identifier: BSD-2-Clause")
    # REUSE-IgnoreEnd
    w("")
    w("//! Oniguruma 6.9.10's Unicode tables, as jq 1.8.1's tarball vendors them")
    w("//! (`vendor/oniguruma/src`). Written by `tools/onig-tables.py`; regenerate")
    w("//! rather than edit.")
    w("#![allow(")
    w("    clippy::unreadable_literal,")
    w("    clippy::upper_case_acronyms,")
    w("    reason = \"generated: Oniguruma's tables and names, as Oniguruma writes them\"")
    w(")]")
    w("")

    c = read(src, "unicode_property_data.c")
    ranges, order, names = properties(c)
    w("/// Each property's code ranges, indexed by Oniguruma's ctype.")
    w(f"pub static CODE_RANGES: [&[(u32, u32)]; {len(order)}] = [")
    for name in order:
        w(f"    &{name.upper()},")
    w("];")
    w("")
    size = re.search(r"#define PROPERTY_NAME_MAX_SIZE\s+(\d+)", c).group(1)
    w("/// The longest property name Oniguruma reads, plus one.")
    w(f"pub const PROPERTY_NAME_MAX_SIZE: usize = {size};")
    w("")
    w("/// Property names, lowercase and without `' '`, `'-'`, `'_'`, and their")
    w("/// ctype; sorted.")
    w(f"pub static PROPERTY_NAMES: [(&str, u16); {len(names)}] = [")
    for name, ctype in names:
        w(f'    ("{name}", {ctype}),')
    w("];")
    w("")
    for name in sorted(set(order)):
        rs = ranges[name]
        w(f"static {name.upper()}: [(u32, u32); {len(rs)}] = {ranges_lit(rs)};")
    w("")

    f = read(src, "unicode_fold_data.c")
    for n in (1, 2, 3):
        rows = sorted((code, key) for key, codes in folds(f, n) for code in codes)
        ty = "u32" if n == 1 else f"[u32; {n}]"
        w(f"/// A code point and what it folds to, {n} code point{'s' if n > 1 else ''}; sorted.")
        w(f"pub static FOLD{n}: [(u32, {ty}); {len(rows)}] = [")
        for code, key in rows:
            k = f"0x{key[0]:x}" if n == 1 else "[" + ", ".join(f"0x{x:x}" for x in key) + "]"
            w(f"    (0x{code:x}, {k}),")
        w("];")
        w("")

    u = read(src, "unicode.c")
    iso = re.search(r"EncUNICODE_ISO_8859_1_CtypeTable\[256\] = \{(.*?)\};", u, re.S).group(1)
    iso = [int(x, 16) for x in re.findall(r"0x([0-9a-f]+)", iso)]
    assert len(iso) == 256
    w("/// The character types of the codes below 256, one bit per standard ctype")
    w("/// (`EncUNICODE_ISO_8859_1_CtypeTable`).")
    w("pub static ISO_8859_1_CTYPE: [u16; 256] = [")
    for i in range(0, 256, 8):
        w("    " + ", ".join(f"0x{v:04x}" for v in iso[i : i + 8]) + ",")
    w("];")
    w("")

    classes, rows = egcb(read(src, "unicode_egcb_data.c"))
    w("/// Grapheme cluster break classes (`EGCB_*`).")
    w("#[derive(Clone, Copy, Debug, PartialEq, Eq)]")
    w("pub enum Egcb {")
    for k in classes:
        w(f"    {k.replace('_', '')},")
    w("}")
    w("")
    w("/// Code ranges and their break class; sorted. Anything else is `Other`.")
    w(f"pub static EGCB: [(u32, u32, Egcb); {len(rows)}] = [")
    for a, b, k in rows:
        w(f"    (0x{a:x}, 0x{b:x}, Egcb::{k.replace('_', '')}),")
    w("];")
    sys.stdout.write("\n".join(o) + "\n")


main()
