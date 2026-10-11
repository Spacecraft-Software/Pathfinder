// SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
// SPDX-License-Identifier: GPL-3.0-or-later

//! jq-source definitions injected ahead of the user's filter.
//!
//! # How this list was decided
//!
//! Not from the jq manual, and not from memory. Every one of jq 1.8.1's 226
//! builtins was invoked against jaq 3.0.0 at its declared arity; the 22 that
//! answered `undefined filter` are the gap, and this module closes the subset
//! of that gap which is expressible in the jq language.
//!
//! The inverse check matters just as much. `leaf_paths`, `ascii`, `isvalid`,
//! `toarray`, `ANY`, `ALL`, `GROUP_BY`, `UNIQUE_BY` and the `@base32` formats
//! are **deliberately absent** — jq 1.8.1 does not define them either (several
//! were removed after 1.6). Polyfilling them would let a filter run here and
//! fail on a machine with real jq, which is the one failure mode a
//! compatibility shim must never introduce.
//!
//! # Two kinds of definition
//!
//! *Polyfills* supply a name jaq does not have. *Repairs* shadow a name jaq
//! *does* have, because jaq's version behaves differently from jq's. Repairs
//! are the riskier category and each carries its measured justification.
//!
//! # Two rules every definition here obeys
//!
//! - **Reach the original through an alias, never by name.** Inside its own
//!   body, a shadowing definition that calls the name it shadows calls *itself*
//!   — infinite recursion under jaq, not a super-call. But definitions bind
//!   names where they are written, so an alias defined *before* the shadow
//!   (`def _pf_mktime: mktime; def mktime: … | _pf_mktime;`) still reaches the
//!   builtin. Every repair that needs the original uses that pattern, and
//!   emits the alias and the shadow together, in that order.
//! - **Prepended, never appended.** Under both jq and jaq the *last* definition
//!   of a name wins, so a user's own `def tostream:` in the filter silently
//!   overrides anything here. That is the desired precedence, and it is why no
//!   collision check is needed.

use std::fmt::Write as _;

/// jq 1.8.1's `builtins` output, captured with `jq -nc 'builtins'`.
///
/// Captured unsorted, in jq's own order: `builtins|sort == builtins` is `false`
/// under jq, and a sorted copy would answer `true`.
///
/// Deliberately jq's list rather than a description of what actually resolves
/// under Pathfinder: scripts call `builtins` to feature-probe *jq*, so jq's
/// answer is the compatible one.
const JQ_BUILTINS: &str = include_str!("embed/jq-builtins.json");

/// The body of an auto-vivifying `setpath`, and the reason this module exists.
///
/// jaq refuses to create missing containers along a path: `null | setpath(["a"];1)`
/// and `{} | setpath(["a","b"];1)` both fail with `cannot use null as iterable`,
/// where jq returns `{"a":1}` and `{"a":{"b":1}}`. Several jq builtins are
/// written on top of that behaviour, so without this helper they cannot be
/// polyfilled at all.
///
/// Only `null` is widened. Every key is checked against its container the way
/// jq's `jv_set` checks it — a wrong-typed key errors with jq's message rather
/// than letting jaq's own `.[$k] = v` build an object with a non-string key,
/// which is not JSON. Slice keys (`{start, end}`) replace that slice with an
/// array, and fractional array indices truncate, as in jq.
///
/// It also reproduces jq's two array-index errors. A negative index reaching
/// before the start of the array is `Out of bounds negative array index`, and
/// an index above `INT_MAX >> 2` (536870911, the limit in jq's `jv_array_set`)
/// is `Array index too large`. Without the second guard, `.[999999999] = 0`
/// would pad a billion-element array instead of erroring.
const SETPATH_BODY: &str = concat!(
    "if ($p|length) == 0 then $v else ($p[0]) as $k | ($k|type) as $kt ",
    // The container, created from null by key type, or the type error jq gives.
    "| (if . == null then (if $kt == \"number\" or $kt == \"object\" then [] ",
    "elif $kt == \"string\" then {} else error(\"Cannot index null with \\($kt)\") end) ",
    "elif type == \"object\" then (if $kt == \"string\" then . ",
    "else error(\"Cannot index object with \\($kt)\") end) ",
    "elif type == \"array\" then (if $kt == \"number\" or $kt == \"object\" then . ",
    "else error(\"Cannot update field at \\($kt) index of array\") end) ",
    "elif type == \"string\" and $kt == \"object\" then error(\"Cannot update string slices\") ",
    "else error(\"Cannot index \\(type) with \\($kt)\") end) as $c ",
    // A slice key: both bounds must be numbers; the slice is replaced by an array.
    "| if $kt == \"object\" then ",
    "(if ($k|has(\"start\") and has(\"end\")) and ([$k.start, $k.end] | all(type == \"number\" or type == \"null\")) | not ",
    "then error(\"Array/string slice indices must be integers\") else . end) ",
    "| _pf_slice($k; $c|length) as [$s, $e] ",
    "| (if ($p|length) == 1 then $v else ($c[$s:$e] | _pf_setpath($p[1:]; $v)) end) as $new ",
    "| (if ($new|type) != \"array\" then error(\"A slice of an array can only be assigned another array\") else . end) ",
    "| $c[:$s] + $new + $c[$e:] ",
    // A number key: truncated to an integer; jq's two index errors; padding.
    "elif $kt == \"number\" then ($k|_pf_toint) as $ki ",
    "| (if $ki < 0 and ($ki + ($c|length)) < 0 then error(\"Out of bounds negative array index\") else . end) ",
    "| (if $ki > 536870911 then error(\"Array index too large\") else . end) ",
    "| (if $ki >= ($c|length) then $c + [range($c|length; $ki+1) | null] else $c end) as $d ",
    "| $d | .[$ki] = (($d|.[$ki]) | _pf_setpath($p[1:]; $v)) ",
    // A string key on an object.
    "else $c | .[$k] = (($c|.[$k]) | _pf_setpath($p[1:]; $v)) end end"
);

/// One injectable definition.
struct Def {
    /// The jq-visible name that triggers injection.
    name: &'static str,
    /// Other definitions this one calls, injected first.
    deps: &'static [&'static str],
    /// The definition's jq source, or `None` when it must be built per-run.
    src: Option<&'static str>,
}

/// Names jaq lacks that jq defines, and that the jq language can express.
static POLYFILLS: &[Def] = &[
    // --- streaming -------------------------------------------------------
    // jq's own definition, used verbatim: the recursive inner `def r` inside
    // `path()` compiles and evaluates correctly under jaq.
    Def {
        name: "tostream",
        deps: &[],
        src: Some(
            "def tostream: path(def r: (.[]?|r), .; r) as $p | getpath($p) \
             | reduce path(.[]?) as $q ([$p, .]; [$p+$q]);",
        ),
    },
    // jq's definition with `setpath` swapped for the auto-vivifying helper.
    // jq's verbatim source dies under jaq because it builds its accumulator by
    // setting a path into a `null` field.
    Def {
        name: "fromstream",
        deps: &["_pf_setpath"],
        src: Some(
            "def fromstream(i): {x: null, e: false} as $init | foreach i as $i ($init; \
             if .e then $init else . end | if $i|length == 2 \
             then _pf_setpath([\"e\"]; $i[0]|length==0) | _pf_setpath([\"x\"]+$i[0]; $i[1]) \
             else _pf_setpath([\"e\"]; $i[0]|length==1) end; \
             if .e then .x else empty end);",
        ),
    },
    Def {
        name: "truncate_stream",
        deps: &[],
        src: Some(
            "def truncate_stream(stream): . as $n | null | stream | . as $input \
             | if (.[0]|length) > $n then setpath([0];$input[0][$n:]) else empty end;",
        ),
    },
    // --- SQL-ish ---------------------------------------------------------
    Def {
        name: "IN",
        deps: &[],
        src: Some("def IN(s): any(s == .; .); def IN(src; s): any(s == src; .);"),
    },
    Def {
        name: "INDEX",
        deps: &[],
        src: Some(
            "def INDEX(stream; idx_expr): reduce stream as $row ({}; .[$row|idx_expr|tostring] = $row); \
             def INDEX(idx_expr): INDEX(.[]; idx_expr);",
        ),
    },
    Def {
        name: "JOIN",
        deps: &[],
        src: Some(
            "def JOIN($idx; idx_expr): [.[] | [., $idx[idx_expr]]]; \
             def JOIN($idx; stream; idx_expr): stream | [., $idx[idx_expr]]; \
             def JOIN($idx; stream; idx_expr; join_expr): stream | [., $idx[idx_expr]] | join_expr;",
        ),
    },
    // --- strings ---------------------------------------------------------
    Def {
        name: "trimstr",
        deps: &[],
        src: Some("def trimstr($val): ltrimstr($val)|rtrimstr($val);"),
    },
    // jq's `format` dispatches an @-format by name. The error wording matches
    // jq's so an unsupported name reads identically.
    Def {
        name: "format",
        deps: &["_pf_base64d", "_pf_urid"],
        src: Some(
            "def format($fmt): if $fmt == \"text\" then @text elif $fmt == \"json\" then @json \
             elif $fmt == \"csv\" then @csv elif $fmt == \"tsv\" then @tsv \
             elif $fmt == \"html\" then @html elif $fmt == \"uri\" then @uri \
             elif $fmt == \"sh\" then @sh elif $fmt == \"base64\" then @base64 \
             elif $fmt == \"base64d\" then _pf_base64d elif $fmt == \"urid\" then _pf_urid \
             else error($fmt + \" is not a valid format\") end;",
        ),
    },
    // --- capability probes -----------------------------------------------
    // Both answer `true`, which is what the jq 1.8.1 baseline answers and what
    // jaq's observed behaviour supports: jaq round-trips
    // `100000000000000000000000000001` and `1.0000000000000000005` unchanged,
    // and keeps them exact through arithmetic (where jq itself falls back to a
    // double and prints `1e+29`). Scripts use these to gate a
    // precision-sensitive branch; answering `false` would push them onto a
    // lossy path that jaq does not actually need.
    Def {
        name: "have_decnum",
        deps: &[],
        src: Some("def have_decnum: true;"),
    },
    Def {
        name: "have_literal_numbers",
        deps: &[],
        src: Some("def have_literal_numbers: true;"),
    },
    // Built per-run; see `dynamic`.
    Def {
        name: "builtins",
        deps: &[],
        src: None,
    },
    Def {
        name: "input_filename",
        deps: &[],
        src: None,
    },
];

/// Definitions that shadow a working jaq builtin to restore jq's behaviour.
static REPAIRS: &[Def] = &[
    // jaq's `setpath` will not auto-vivify; jq's will. Provably jq-identical
    // (verified against jq 1.8.1 across null, missing, existing and
    // wrong-typed containers) and scoped to calls the user actually wrote.
    Def {
        name: "setpath",
        deps: &["_pf_setpath"],
        src: Some("def setpath($p; $v): _pf_setpath($p; $v);"),
    },
    // jaq renders fractional epoch seconds as `1970-01-01T00:00:01.5Z`;
    // jq truncates to whole seconds. Depends on the `strftime` repair so that a
    // short broken-down-time array (`[2024,2,15] | todate`) works as in jq; the
    // dependency also fixes the emission order, which is what decides that
    // `strftime` here binds the repair rather than the builtin.
    Def {
        name: "todateiso8601",
        deps: &["strftime"],
        src: Some(
            "def todateiso8601: (if type == \"number\" then floor else . end) \
             | strftime(\"%Y-%m-%dT%H:%M:%SZ\");",
        ),
    },
    Def {
        name: "todate",
        deps: &["todateiso8601"],
        src: Some("def todate: todateiso8601;"),
    },
    // --- deletion -----------------------------------------------------------
    // jaq's `delpaths` deletes in the order given, so an earlier deletion
    // shifts the indices of later ones: `["a","b","c"] | del(.[0,1])` is
    // `["b"]` under jaq and `["c"]` under jq. It also errors on out-of-range
    // indices and `null` intermediates, which jq ignores, and outputs nothing
    // for `delpaths([[]])`. These shadows run jq's algorithm (`jv_delpaths` in
    // jq's src/jv_aux.c): sort, group by leading key, recurse, and delete a
    // container's keys all at once against its original length.
    Def {
        name: "delpaths",
        deps: &["_pf_delpaths"],
        src: Some("def delpaths($ps): _pf_delpaths($ps);"),
    },
    Def {
        name: "del",
        // `_pf_del0` is not used here; depending on it guarantees the alias of
        // jaq's own `del` is emitted before this shadow, whichever comes first.
        deps: &["_pf_del0", "_pf_delpaths"],
        src: Some("def del(f): _pf_delpaths([path(f)]);"),
    },
    // --- regex ------------------------------------------------------------
    // jq's `scan` is always global: its definition matches with `"g" + $flags`.
    // jaq's `scan/1` returns only the first match (`"abcdc" | [scan("c")]` is
    // `["c"]`, not `["c","c"]`). jq 1.8.1's definition, verbatim, over the
    // repaired `match`, so an unmatched group scans as `null` as in jq.
    Def {
        name: "scan",
        deps: &["match"],
        src: Some(
            "def scan($re; $flags): match($re; \"g\" + $flags) \
             | if (.captures|length > 0) then [ .captures | .[] | .string ] else .string end; \
             def scan($re): scan($re; null);",
        ),
    },
    // --- generators -------------------------------------------------------
    // jq's `nth($n; g)` takes `$n` as a value parameter, so `nth(1,2; g)`
    // yields one result per index. jaq's native `nth` uses only the first.
    // jq 1.8.1's definition, verbatim (`first` and `skip` are not shadowed).
    Def {
        name: "nth",
        deps: &[],
        src: Some(
            "def nth($n; g): if $n < 0 then error(\"nth doesn't support negative indices\") \
             else first(skip($n; g)) end;",
        ),
    },
    // jq's `limit`/`skip` reject a negative count; jaq's return nothing (or
    // everything). jq 1.8.1's definitions, verbatim.
    Def {
        name: "limit",
        deps: &[],
        src: Some(
            "def limit($n; expr): if $n > 0 then label $out | foreach expr as $item \
             ($n; . - 1; $item, if . <= 0 then break $out else empty end) \
             elif $n == 0 then empty else error(\"limit doesn't support negative count\") end;",
        ),
    },
    Def {
        name: "skip",
        deps: &[],
        src: Some(
            "def skip($n; expr): if $n > 0 then foreach expr as $item \
             ($n; . - 1; if . < 0 then $item else empty end) \
             elif $n == 0 then expr else error(\"skip doesn't support negative count\") end;",
        ),
    },
    // --- collections --------------------------------------------------------
    // jq rejects a negative depth; jaq treats it as unlimited.
    Def {
        name: "flatten",
        deps: &[],
        src: Some(
            "def _pf_flatten($x): reduce .[] as $i ([]; if $i | type == \"array\" and $x != 0 \
             then . + ($i | _pf_flatten($x - 1)) else . + [$i] end); \
             def flatten($x): if $x < 0 then error(\"flatten depth must not be negative\") \
             else _pf_flatten($x) end; def flatten: _pf_flatten(-1);",
        ),
    },
    // jq joins `null` as the empty string; jaq writes `null`.
    Def {
        name: "join",
        deps: &[],
        src: Some(
            "def join($x): reduce .[] as $i (null; (if . == null then \"\" else . + $x end) \
             + ($i | if type == \"boolean\" or type == \"number\" then tostring else . // \"\" end)) \
             // \"\";",
        ),
    },
    // jaq's `pick` on an array index builds an object with a *number* key —
    // `[1,2,3] | pick(.[1])` prints `{1:2}`, which is not JSON. jq's
    // definition, over the auto-vivifying setpath, gives `[null,2]`.
    Def {
        name: "pick",
        deps: &["_pf_setpath"],
        src: Some(
            "def pick(pathexps): . as $in | reduce path(pathexps) as $a (null; \
             _pf_setpath($a; $in | getpath($a)));",
        ),
    },
    // jaq's `from_entries` reads only `key`/`k`/`name` and `value`/`v`, and
    // builds an object around whatever key it finds — `[{"key":null,
    // "value":1}]` prints `{null:1}`, which is not JSON. jq 1.8.1 also accepts
    // `Key`/`Name`/`Value` and rejects a non-string key. jq's definition
    // (`map({(k): v}) | add | .//={}`), as one pass that sets each key in turn,
    // with the key check jq's object construction makes. The type test uses
    // comparisons — under jaq, `type == "string"` costs several times more.
    Def {
        name: "from_entries",
        deps: &["_pf_dump"],
        src: Some(
            "def from_entries: reduce .[] as $__pf_e ({}; \
             ($__pf_e | .key // .Key // .name // .Name) as $__pf_k \
             | if $__pf_k < \"\" or $__pf_k >= [] \
             then error(\"Cannot use \\($__pf_k|type) (\\($__pf_k|_pf_dump)) as object key\") \
             else .[$__pf_k] = ($__pf_e | if has(\"value\") then .value else .Value end) end);",
        ),
    },
    // jaq's native `with_entries` reaches jaq's own `from_entries`, not the
    // repair above. jq 1.8.1's definition, verbatim.
    Def {
        name: "with_entries",
        deps: &["from_entries"],
        src: Some("def with_entries(f): to_entries | map(f) | from_entries;"),
    },
    // jaq's `has` answers `true` for a negative array index and `false` for a
    // number key on an object; jq answers `false` and raises an error. jaq
    // also rejects `has(nan)` and `has(1.5)`, which jq answers. jq's
    // `jv_has`: `null` has nothing, an object takes a string key, an array a
    // number key (NaN is absent, a fraction truncates, a negative index is
    // absent), anything else is an error. A string key goes to jaq's builtin, which is right
    // wherever it does not raise an error; everything else takes the exact
    // path. Measured on `select(has("a"))` over 300,000 objects: 0.36 s
    // against jaq's 0.28 s; the same guard written with `type ==` took 2.8 s.
    Def {
        name: "has",
        deps: &[],
        src: Some(
            "def _pf_has0($k): has($k); \
             def _pf_has_err($k): error(\"Cannot check whether \\(type) has a \\($k|type) key\"); \
             def _pf_has_slow($k): if . == null then false elif . >= {} then \
             (if $k >= \"\" then (if $k < [] then _pf_has0($k) else _pf_has_err($k) end) else _pf_has_err($k) end) \
             elif . >= [] then (if $k > true then (if $k < \"\" then \
             (if ($k|isnan) then false else ($k | if . < 0 then ceil else floor end) as $i \
             | $i >= 0 and $i < length end) else _pf_has_err($k) end) else _pf_has_err($k) end) \
             else _pf_has_err($k) end; \
             def has($k): if $k < \"\" then _pf_has_slow($k) elif $k >= [] then _pf_has_slow($k) \
             else . as $__pf_in | try _pf_has0($k) catch ($__pf_in | _pf_has_slow($k)) end;",
        ),
    },
    // jaq's `implode` rejects a fractional codepoint and one outside Unicode;
    // jq truncates the first and writes U+FFFD for the second (and for a
    // surrogate), and words its errors differently. jaq's builtin answers
    // every array it accepts the way jq does, so it runs first.
    Def {
        name: "implode",
        deps: &["_pf_dump"],
        src: Some(
            "def _pf_implode0: implode; \
             def implode: if . >= [] then (if . < {} then . as $__pf_a | try _pf_implode0 \
             catch ($__pf_a | map(if . > true then (if . < \"\" then (if isnan then null else . end) else null end) else null end \
             // error(\"\\(type) (\\(_pf_dump)) can't be imploded, unicode codepoint needs to be numeric\") \
             | if . < 0 then ceil else floor end \
             | if . < 0 or . > 1114111 or (. >= 55296 and . <= 57343) then 65533 else . end) \
             | _pf_implode0) \
             else error(\"implode input must be an array\") end) \
             else error(\"implode input must be an array\") end;",
        ),
    },
    // jq accepts exactly `true`, `false`, `"true"` and `"false"`. jaq parses
    // the string as JSON, so `" true"` is accepted, and its errors differ.
    Def {
        name: "toboolean",
        deps: &["_pf_dump"],
        src: Some(
            "def toboolean: if . == true or . == false then . elif . == \"true\" then true \
             elif . == \"false\" then false \
             else error(\"\\(type) (\\(_pf_dump)) cannot be parsed as a boolean\") end;",
        ),
    },
    // --- strings ------------------------------------------------------------
    // jq's `startswith`/`endswith` reject a non-string with their own message,
    // and jq 1.8.1 defines `ltrimstr`/`rtrimstr` on top of them, so all four
    // report the same error jq does.
    Def {
        name: "startswith",
        deps: &[],
        src: Some(
            "def _pf_startswith($s): startswith($s); def startswith($s): \
             if type == \"string\" and ($s|type) == \"string\" then _pf_startswith($s) \
             else error(\"startswith() requires string inputs\") end;",
        ),
    },
    Def {
        name: "endswith",
        deps: &[],
        src: Some(
            "def _pf_endswith($s): endswith($s); def endswith($s): \
             if type == \"string\" and ($s|type) == \"string\" then _pf_endswith($s) \
             else error(\"endswith() requires string inputs\") end;",
        ),
    },
    // jaq's `tonumber` parses the string as a stream of JSON values: `"1a"`
    // yields `1` and then an error, `""` yields nothing, `" 4"` is accepted.
    // jq 1.8.1 accepts exactly an optional sign, digits with an optional point
    // (or a leading one), an optional exponent, and `nan`/`infinity`/`inf`
    // in any case. A string jaq's `tonumber` reads in full — its first number prints
    // back as the string — is already right; anything else takes the exact
    // path, which normalises valid strings to JSON's spelling (no `+`, no bare
    // point) and raises jq's error for the rest — including the strings jaq
    // reads as no value at all (`""`), which fall past the label. A float
    // keeps its literal text in jaq, sign included, so `"+5.43"` would print
    // back as itself — and as invalid JSON — and is sent the exact way. Measured on
    // `map(tonumber)`: the exact path alone ran 35x slower than jq; this shape
    // costs 1.5x jaq's own `tonumber`.
    // A string that cannot start a number (`"N/A"`, `"abc"`, `""`) is rejected
    // on its first character, before the regex, which jaq compiles on every
    // call. Type tests are comparisons: under jaq 3.1 `type == "string"` costs
    // about 4 µs, and the first version ran it up to six times per call
    // (`try tonumber catch null` over `"N/A"`: 57 µs each).
    Def {
        name: "tonumber",
        deps: &["_pf_dump"],
        src: Some(concat!(
            r#"def _pf_tonumber0: tonumber; "#,
            r#"def _pf_tonumber_err: error("\(type) (\(_pf_dump)) cannot be parsed as a number"); "#,
            r#"def _pf_tonumber_serr: error("string (\(_pf_dump)) cannot be parsed as a number"); "#,
            r#"def _pf_tonumber_exact: if . < "" then _pf_tonumber_err elif . >= [] then _pf_tonumber_err "#,
            r#"else .[:1] as $c | if $c > "9" then (ascii_downcase as $l | if $l == "nan" then nan "#,
            r#"elif $l == "infinity" then infinite elif $l == "inf" then infinite else _pf_tonumber_serr end) "#,
            r#"elif $c < "+" then _pf_tonumber_serr elif $c == "," then _pf_tonumber_serr "#,
            r#"elif $c == "/" then _pf_tonumber_serr "#,
            r#"elif test("^[+-]?([0-9]+[.]?[0-9]*|[.][0-9]+)([eE][+-]?[0-9]+)?$") then "#,
            r#"(if .[:1] == "+" then .[1:] else . end) "#,
            r#"| (if .[:1] == "-" then "-" else "" end) as $s | .[($s | length):] "#,
            r#"| split(".") as $p "#,
            r#"| (if ($p | length) == 1 then $p[0] "#,
            r#"elif $p[1] == "" or ($p[1][:1] | . == "e" or . == "E") then (if $p[0] == "" then "0" else $p[0] end) + $p[1] "#,
            r#"else (if $p[0] == "" then "0" else $p[0] end) + "." + $p[1] end) "#,
            r#"| $s + . | _pf_tonumber0 "#,
            r#"else ascii_downcase as $l | if $l == "-nan" then nan elif $l == "+nan" then nan "#,
            r#"elif $l == "+infinity" then infinite elif $l == "-infinity" then 0 - infinite "#,
            r#"elif $l == "+inf" then infinite elif $l == "-inf" then 0 - infinite "#,
            r#"else _pf_tonumber_serr end end end; "#,
            // Nested `if`s, not `and`: each `and` costs jaq several µs.
            r#"def tonumber: label $__pf_o "#,
            r#"| ((try _pf_tonumber0 catch null) as $__pf_r "#,
            r#"| if $__pf_r > true then (if $__pf_r < "" then (($__pf_r | tostring) as $__pf_t "#,
            r#"| if $__pf_t == tostring then (if $__pf_t[:1] != "+" then $__pf_r, break $__pf_o "#,
            r#"else _pf_tonumber_exact, break $__pf_o end) else _pf_tonumber_exact, break $__pf_o end) "#,
            r#"else _pf_tonumber_exact, break $__pf_o end) else _pf_tonumber_exact, break $__pf_o end), "#,
            r#"_pf_tonumber_exact;"#,
        )),
    },
    Def {
        name: "ltrimstr",
        deps: &["startswith"],
        src: Some("def ltrimstr($left): if startswith($left) then .[$left | length:] else . end;"),
    },
    Def {
        name: "rtrimstr",
        deps: &["endswith"],
        src: Some(
            "def rtrimstr($right): if endswith($right) then .[:$right | -length] else . end;",
        ),
    },
    // jq's one-argument regex forms accept `[re, flags]` as well as a string.
    // jaq's accept only a string. jq 1.8.1's definitions, delegating through
    // aliases — but where jq passes `null` flags, these call jaq's one-argument
    // builtin instead: jaq's two-argument forms reject `null` ("cannot use null
    // as string"), which broke plain `match("re")` in the first version of this
    // repair.
    // jaq leaves an unmatched group out of `captures` altogether and gives an
    // unnamed group no `name`; jq lists every group in order, unmatched ones
    // as `{"offset":-1,"string":null,"length":0,"name":…}`, and names unnamed
    // ones `null`. Once a group is missing, jaq's list no longer says which
    // group each entry was, so every unnamed capture group is given a
    // synthetic name (`(?<__pf_g0>`) before matching, and the list is rebuilt
    // in group order. Group openers are found with one native match over the
    // regex text that steps over escapes, character classes and `(?…)`. A
    // regex without `(` is matched as it is.
    Def {
        name: "match",
        deps: &["_pf_regex_lit"],
        src: Some(concat!(
            r#"def _pf_match0(re; mode): match(re; mode); def _pf_match01(re): match(re); "#,
            // What is known about a regex: the table for a literal (from
            // `src/regex.rs`), or the run-time scan, which assumes the worst.
            // Extended mode (`x`) changes what the text means, so it always
            // takes the scan.
            r#"def _pf_scanre($re): if ($re | contains("(")) | not "#,
            r#"then {re: $re, simple: true, unnamed: false, nullable: null} "#,
            r#"else reduce ($re | _pf_match0("\\\\.|\\[\\^?\\]?(?:\\[:[a-z]+:\\]|\\\\.|[^\\]\\\\])*\\]"#,
            r#"|\\(\\?P?<[A-Za-z_][A-Za-z0-9_]*>|\\(\\?|\\("; "g")) as $t "#,
            r#"({re: "", at: 0, groups: [], dict: {}, simple: false, nullable: null}; "#,
            r#"if $t.string == "(" then ("__pf_g\(.groups | length)") as $n "#,
            r#"| .re += $re[.at:$t.offset] + "(?<\($n)>" | .at = $t.offset + 1 | .groups += [[null, $n]] "#,
            r#"| .dict[$n] = null "#,
            r#"elif ($t.string | .[:3] == "(?<" or .[:4] == "(?P<") "#,
            r#"then ($t.string | if .[2:3] == "P" then .[4:-1] else .[3:-1] end) as $n "#,
            r#"| .groups += [[$n, $n]] | .dict[$n] = $n else . end) | .re += $re[.at:] end; "#,
            r#"def _pf_regex($re; $mode): if $mode == null then ($__pf_rl[$re] // _pf_scanre($re)) "#,
            r#"elif ($mode | contains("x")) then _pf_scanre($re) else ($__pf_rl[$re] // _pf_scanre($re)) end; "#,
            // jaq's global matching drops an empty match that starts where a
            // non-empty one ended (`"ab1" | match("[a-z]*"; "g")` misses the
            // one at 2); jq reports it. Where the regex can match empty, each
            // such position is re-tried with one character of left context,
            // which is all a Rust-regex assertion looks at.
            r#"def _pf_gaps($re; $mode): . as $s | ($mode | explode | map(select(. != 103)) | implode) as $m1 "#,
            r#"| [_pf_match0($re; $mode)] as $ms | range($ms | length) as $i | $ms[$i] "#,
            r#"| ., (if .length > 0 and ($i + 1 == ($ms | length) or $ms[$i + 1].offset > .offset + .length) "#,
            r#"then (.offset + .length) as $e | first($s[$e - 1:] | _pf_match0("\\A(?s:.)(?:" + $re + ")"; $m1) "#,
            r#"| select(.length == 1) | {offset: $e, length: 0, string: "", "#,
            r#"captures: (.captures | map(.offset += $e - 1))})? // empty else empty end); "#,
            r#"def _pf_rawg($r; $mode): if $mode == null then _pf_match01($r.re) "#,
            r#"elif ($mode | contains("g")) then (if $r.nullable == false then _pf_match0($r.re; $mode) "#,
            r#"elif ($mode | contains("n")) then _pf_match0($r.re; $mode) "#,
            r#"elif $r.nullable then _pf_gaps($r.re; $mode) "#,
            r#"elif (first($r.re | _pf_match01("\\\\[bBAzZ]|[$^]")) | true) "#,
            r#"// ([("" | _pf_match0("\\A(?:" + $r.re + ")\\z"; ""))] | length > 0) "#,
            r#"then _pf_gaps($r.re; $mode) else _pf_match0($r.re; $mode) end) "#,
            r#"else _pf_match0($r.re; $mode) end; "#,
            // Every group takes part: jaq's list is right but for the `name`
            // of an unnamed group. Otherwise rename where every group matched,
            // and rebuild the list in group order where some did not.
            r#"def _pf_match($re; $mode): _pf_regex($re; $mode) as $r | _pf_rawg($r; $mode) "#,
            r#"| if $r.simple then (if $r.unnamed then .captures |= map(if .name == null then .name = null else . end) "#,
            r#"else . end) "#,
            r#"elif (.captures | length) == ($r.groups | length) then .captures |= map(.name = $r.dict[.name]) "#,
            r#"else (reduce .captures[] as $c ({}; .[$c.name] = $c)) as $by "#,
            r#"| .captures = [$r.groups[] as [$name, $syn] | $by[$syn] as $m "#,
            r#"| if $m == null then {offset: -1, string: null, length: 0, name: $name} "#,
            r#"else {offset: $m.offset, length: $m.length, string: $m.string, name: $name} end] end; "#,
            // jq's `$`-parameter order: the mode is the outer loop.
            r#"def match(re; mode): mode as $mode | re as $re | _pf_match($re; $mode); "#,
            // jq's dispatch on `$val`'s type, with comparisons for `type ==`,
            // which costs jaq several microseconds a call.
            r#"def match($val): if $val < [] then (if $val >= "" then _pf_match($val; null) "#,
            r#"else error("\($val|type) not a string or array") end) "#,
            r#"elif $val < {} then (if ($val | length) > 1 then _pf_match($val[0]; $val[1]) "#,
            r#"elif ($val | length) > 0 then _pf_match($val[0]; null) "#,
            r#"else error("array not a string or array") end) "#,
            r#"else error("object not a string or array") end;"#,
        )),
    },
    Def {
        name: "test",
        deps: &[],
        src: Some(concat!(
            r#"def _pf_test(re; mode): test(re; mode); def _pf_test1(re): test(re); "#,
            r#"def test($val): if $val < [] then (if $val >= "" then _pf_test1($val) "#,
            r#"else error("\($val|type) not a string or array") end) "#,
            r#"elif $val < {} then (if ($val | length) > 1 then _pf_test($val[0]; $val[1]) "#,
            r#"elif ($val | length) > 0 then _pf_test1($val[0]) "#,
            r#"else error("array not a string or array") end) "#,
            r#"else error("object not a string or array") end;"#,
        )),
    },
    // jq 1.8.1's `capture`, `sub` and `gsub`, verbatim, over the repaired
    // `match`: an unmatched named group is `null` in `capture` (jaq leaves the
    // key out), and `sub`/`gsub` follow jq on empty matches and on a
    // replacement with several outputs, where jaq's own differ. One change to
    // `sub`: jq grows `.result` with `.result[$ix] += …`, which jaq's `+=`
    // refuses past the end, so the array is extended explicitly.
    Def {
        name: "capture",
        deps: &["match"],
        src: Some(concat!(
            // Where every group takes part, jaq's own `capture` is jq's.
            r#"def _pf_capture0(re; mods): capture(re; mods); def _pf_capture01(re): capture(re); "#,
            r#"def _pf_jqcapture($re; $mods): match($re; $mods) | reduce (.captures | .[] | select(.name != null) "#,
            r#"| { (.name) : .string }) as $pair ({}; . + $pair); "#,
            r#"def capture(re; mods): mods as $mods | re as $re | if $re < "" then _pf_jqcapture($re; $mods) "#,
            r#"elif _pf_regex($re; $mods).simple then (if $mods == null then _pf_capture01($re) "#,
            r#"else _pf_capture0($re; $mods) end) else _pf_jqcapture($re; $mods) end; "#,
            r#"def capture($val): if $val < [] then (if $val >= "" then capture($val; null) "#,
            r#"else error("\($val|type) not a string or array") end) "#,
            r#"elif $val < {} then (if ($val | length) > 1 then capture($val[0]; $val[1]) "#,
            r#"elif ($val | length) > 0 then capture($val[0]; null) "#,
            r#"else error("array not a string or array") end) "#,
            r#"else error("object not a string or array") end;"#,
        )),
    },
    // jq's `sub` is slow in the jq language; jaq's builtin gives jq's
    // answer whenever every group takes part, the regex cannot match empty,
    // and the replacement yields exactly one value per match. The first two
    // are known for a literal regex; the third is checked as it runs, and a
    // replacement with several outputs (or none) restarts on jq's definition.
    Def {
        name: "sub",
        deps: &["match"],
        src: Some(concat!(
            r#"def _pf_sub0(re; s; flags): sub(re; s; flags); "#,
            r#"def _pf_jqsub($re; s; $flags): . as $in | (reduce match($re; $flags) as $edit "#,
            r#"({result: [], previous: 0}; $in[ .previous: ($edit | .offset) ] as $gap "#,
            r#"| [reduce ( $edit | .captures | .[] | select(.name != null) | { (.name) : .string } ) as $pair "#,
            r#"({}; . + $pair) | s ] as $inserts "#,
            r#"| reduce range(0; $inserts|length) as $ix (.; .result |= (if length > $ix "#,
            r#"then .[$ix] += $gap + $inserts[$ix] else . + [range(length; $ix) | null] + [$gap + $inserts[$ix]] end)) "#,
            r#"| .previous = ($edit | .offset + .length ) ) | .result[] + $in[.previous:] ) // $in; "#,
            r#"def sub($re; s; $flags): if $re < "" then _pf_jqsub($re; s; $flags) "#,
            r#"elif _pf_regex($re; $flags) | .simple and .nullable == false then . as $__pf_in "#,
            r#"| try _pf_sub0($re; [s] | if length == 1 then .[0] else error("__pf_multi") end; $flags) "#,
            r#"catch (if . == "__pf_multi" then $__pf_in | _pf_jqsub($re; s; $flags) else error end) "#,
            r#"else _pf_jqsub($re; s; $flags) end; "#,
            r#"def sub($re; s): sub($re; s; "");"#,
        )),
    },
    Def {
        name: "gsub",
        deps: &["sub"],
        src: Some(
            r#"def gsub($re; s; flags): sub($re; s; flags + "g"); def gsub($re; s): sub($re; s; "g");"#,
        ),
    },
    // Builtins whose jq error names the builtin, so no translation of jaq's
    // generic message can produce it. jaq's builtin runs first; only an error
    // with an input of the wrong type is re-raised with jq's text.
    Def {
        name: "utf8bytelength",
        deps: &["_pf_dump"],
        src: Some(concat!(
            r#"def _pf_utf8bytelength0: utf8bytelength; def utf8bytelength: . as $__pf_v "#,
            r#"| try _pf_utf8bytelength0 catch (if $__pf_v < "" or $__pf_v >= [] "#,
            r#"then $__pf_v | error("\(type) (\(_pf_dump)) only strings have UTF-8 byte length") "#,
            r#"else error end);"#,
        )),
    },
    Def {
        name: "trim",
        deps: &[],
        src: Some(concat!(
            r#"def _pf_trim0: trim; def trim: . as $__pf_v | try _pf_trim0 "#,
            r#"catch (if $__pf_v < "" or $__pf_v >= [] then error("trim input must be a string") else error end);"#,
        )),
    },
    Def {
        name: "ltrim",
        deps: &[],
        src: Some(concat!(
            r#"def _pf_ltrim0: ltrim; def ltrim: . as $__pf_v | try _pf_ltrim0 "#,
            r#"catch (if $__pf_v < "" or $__pf_v >= [] then error("trim input must be a string") else error end);"#,
        )),
    },
    Def {
        name: "rtrim",
        deps: &[],
        src: Some(concat!(
            r#"def _pf_rtrim0: rtrim; def rtrim: . as $__pf_v | try _pf_rtrim0 "#,
            r#"catch (if $__pf_v < "" or $__pf_v >= [] then error("trim input must be a string") else error end);"#,
        )),
    },
    Def {
        name: "bsearch",
        deps: &["_pf_dump"],
        src: Some(concat!(
            r#"def _pf_bsearch0($t): bsearch($t); def bsearch($t): . as $__pf_v | try _pf_bsearch0($t) "#,
            r#"catch (if $__pf_v < [] or $__pf_v >= {} "#,
            r#"then $__pf_v | error("\(type) (\(_pf_dump)) cannot be searched from") else error end);"#,
        )),
    },
    // --- dates --------------------------------------------------------------
    // jq accepts a broken-down time shorter than eight fields and treats the
    // missing fields as zero: `[2024,2,15] | mktime` is 1710460800. jaq
    // rejects it ("cannot convert [2024,2,15] to time"). Each repair pads, then
    // calls the builtin through an alias defined before the shadow.
    Def {
        name: "mktime",
        deps: &["_pf_pad_tm"],
        src: Some(concat!(
            r#"def _pf_mktime: mktime; def mktime: . as $__pf_a | try (_pf_pad_tm | _pf_mktime) "#,
            r#"catch (if $__pf_a < [] or $__pf_a >= {} then error("mktime requires array inputs") "#,
            r#"elif any($__pf_a[]; type != "number") then error("mktime requires parsed datetime inputs") "#,
            r#"else error end);"#,
        )),
    },
    Def {
        name: "strftime",
        deps: &["_pf_pad_tm"],
        src: Some(concat!(
            r#"def _pf_strftime($f): strftime($f); def strftime($f): . as $__pf_a | try (_pf_pad_tm | _pf_strftime($f)) "#,
            r#"catch (($__pf_a | type) as $t | if $t != "number" and $t != "array" "#,
            r#"then error("strftime/1 requires parsed datetime inputs") "#,
            r#"elif ($f | type) != "string" then error("strftime/1 requires a string format") "#,
            r#"elif $t == "array" and any($__pf_a[]; type != "number") "#,
            r#"then error("strftime/1 requires parsed datetime inputs") else error end);"#,
        )),
    },
    Def {
        name: "strflocaltime",
        deps: &["_pf_pad_tm"],
        src: Some(concat!(
            r#"def _pf_strflocaltime($f): strflocaltime($f); def strflocaltime($f): . as $__pf_a | try (_pf_pad_tm | _pf_strflocaltime($f)) "#,
            r#"catch (($__pf_a | type) as $t | if $t != "number" and $t != "array" "#,
            r#"then error("strflocaltime/1 requires parsed datetime inputs") "#,
            r#"elif ($f | type) != "string" then error("strflocaltime/1 requires a string format") "#,
            r#"elif $t == "array" and any($__pf_a[]; type != "number") "#,
            r#"then error("strflocaltime/1 requires parsed datetime inputs") else error end);"#,
        )),
    },
];

/// Internal helpers, never triggered by name in a user filter.
static INTERNAL: &[Def] = &[
    Def {
        name: "_pf_setpath",
        deps: &["_pf_slice", "_pf_toint"],
        src: None,
    },
    // `(int)` in C truncates toward zero.
    Def {
        name: "_pf_toint",
        deps: &[],
        src: Some("def _pf_toint: if . < 0 then ceil else floor end;"),
    },
    // jq's `parse_slice`: a `{start, end}` key against an array of length
    // `$len`, as `[start, end)`. Start rounds down, end rounds up.
    Def {
        name: "_pf_slice",
        deps: &[],
        src: Some(
            "def _pf_slice($s; $len): (if $s.start == null then 0 else $s.start end) as $a \
             | (if $s.end == null then $len else $s.end end) as $b \
             | if ($a|type) != \"number\" or ($b|type) != \"number\" \
             then error(\"Array/string slice indices must be integers\") \
             else ($a | if isnan then 0 else . end | if . < 0 then . + $len else . end \
             | if . < 0 then 0 elif . > $len then $len else . end | floor) as $start \
             | ($b | if isnan then $len else . end | if . < 0 then . + $len else . end) as $dend \
             | (if $dend < 0 then $start else ($dend | floor) end) as $e0 \
             | ([$e0, $len] | min) as $e1 \
             | (if $e1 < $len and $e1 < $dend then $e1 + 1 else $e1 end) as $e2 \
             | [$start, ([$e2, $start] | max)] end;",
        ),
    },
    // jaq's own `del`, reachable after the repair shadows it.
    Def {
        name: "_pf_del0",
        deps: &[],
        src: Some("def _pf_del0(f): del(f);"),
    },
    // An index or slice deletion with jaq's own `del`, on arrays only: jaq
    // quietly ignores `del(.[0])` on an object, where jq raises an error, so
    // anything else is refused for the rewriter's fallback to answer.
    Def {
        name: "_pf_delat",
        deps: &["_pf_del0"],
        src: Some(
            "def _pf_delat(f): if type == \"array\" then _pf_del0(f) \
             else error(\"pathfinder: not an array\") end;",
        ),
    },
    // Order-preserving deletion of named keys. jaq deletes a key named
    // directly (`del(.a)`) by swapping the last key into its place, so jq's key
    // order is lost; rebuilding keeps it. Anything but an object is refused so
    // the rewriter's fallback can answer as jq does.
    Def {
        name: "_pf_delkeys",
        deps: &[],
        src: Some(
            "def _pf_delkeys($ks): if type == \"object\" then . as $o \
             | if any($ks[]; . as $k | $o | has($k)) \
             then reduce (keys_unsorted[] | select(. as $x | all($ks[]; . != $x))) as $x ({}; .[$x] = $o[$x]) \
             else . end \
             else error(\"pathfinder: not an object\") end;",
        ),
    },
    // jq's `jv_dels`: delete a set of keys from one container, all at once.
    // A few plain indices are deleted natively from the highest down, which is
    // the same as deleting them simultaneously; many indices, or any slice,
    // rebuild the array in one pass through a deletion mask instead (one
    // native delete per index, or a membership scan per element, is quadratic).
    Def {
        name: "_pf_dels",
        deps: &["_pf_toint", "_pf_slice", "_pf_delkeys"],
        src: Some(
            "def _pf_dels($keys): if type == \"null\" or ($keys | length) == 0 then . \
             elif type == \"array\" then length as $len \
             | if ($keys | length) <= 32 and all($keys[]; type == \"number\") then \
             reduce ($keys | map(if . < 0 then $len + (.|_pf_toint) else (.|_pf_toint) end) \
             | unique | reverse[] | select(. >= 0 and . < $len)) as $i (.; del(.[$i])) \
             else . as $a | (reduce $keys[] as $k ([range($len) | false]; \
             if ($k|type) == \"number\" then (if $k < 0 then $len + ($k|_pf_toint) else ($k|_pf_toint) end) as $i \
             | if $i >= 0 and $i < $len then .[$i] = true else . end \
             elif ($k|type) == \"object\" then _pf_slice($k; $len) as [$f, $t] \
             | reduce range($f; $t) as $i (.; .[$i] = true) \
             else error(\"Cannot delete \\($k|type) element of array\") end)) as $m \
             | [range($len) as $i | select($m[$i] | not) | $a[$i]] end \
             elif type == \"object\" then \
             if any($keys[]; type != \"string\") \
             then error(\"Cannot delete \\(first($keys[] | select(type != \"string\")) | type) field of object\") \
             else _pf_delkeys($keys) end \
             else error(\"Cannot delete fields from \\(type)\") end;",
        ),
    },
    // jq's `delpaths_sorted`: group paths by leading key; recurse into groups
    // that go deeper (skipping `null` sub-values), then delete the whole keys
    // via `_pf_dels`. The two sets of keys are disjoint, so the order is free;
    // collecting the keys in one array keeps it linear. A key that just read a
    // non-null value exists, so jaq's own in-place `.[$k] =` is safe for it.
    Def {
        name: "_pf_delsorted",
        deps: &["_pf_dels", "_pf_setpath"],
        src: Some(
            "def _pf_delsorted($ps): ($ps | group_by(.[0])) as $gs \
             | reduce ($gs[] | select(all(.[]; length > 1))) as $g (.; \
             ($g[0][0]) as $k | .[$k] as $sub \
             | if $sub == null then . \
             else ($sub | _pf_delsorted($g | map(.[1:]))) as $new \
             | if ($k|type) == \"string\" or (($k|type) == \"number\" and $k >= 0 and $k == ($k|floor)) \
             then .[$k] = $new else _pf_setpath([$k]; $new) end end) \
             | _pf_dels([$gs[] | select(any(.[]; length == 1)) | .[0][0]]);",
        ),
    },
    // jq's `jv_delpaths`: validate, sort, and handle deleting the root.
    Def {
        name: "_pf_delpaths",
        deps: &["_pf_delsorted"],
        src: Some(
            "def _pf_delpaths($paths): if ($paths|type) != \"array\" \
             then error(\"Paths must be specified as an array\") \
             else ($paths | sort) as $ps \
             | if any($ps[]; type != \"array\") then error(\"Path must be specified as array, not \\(first($ps[] | select(type != \"array\")) | type)\") \
             elif ($ps | length) == 0 then . \
             elif ($ps[0] | length) == 0 then null \
             else _pf_delsorted($ps) end end;",
        ),
    },
    // The pre-pass that lets jaq's own (fast) assignment run with jq's
    // semantics. Measured: where every container on a path already exists,
    // jaq's native `=`, `|=` and `op=` agree with jq — `|= empty`, multiple
    // outputs, slices. They differ only where a container is missing, an array
    // index is past the end, or a key is the wrong type for its container. This
    // walks each path the assignment will touch and fixes exactly those,
    // without touching the leaf value: `null` containers become `[]`/`{}` by
    // key type, short arrays are padded with `null`, and a bad key raises jq's
    // own error. Writes happen only where something is missing, so on data
    // that needs nothing it reads paths and passes the document through. The
    // common case — the leaf's parent exists, has the right type, and the
    // index is in range — is decided by one `getpath` and a type test; only
    // the rest walks the path level by level. The check is written inline
    // rather than as a helper: calling a jq-defined function costs jaq about
    // 15 µs, which dominated the whole pre-pass (2 s for 100,000 elements
    // with a helper, under 0.7 s inline — faster than jq itself).
    //
    // Why not jq's `_modify`: jq writes it with a builtin-private `$$$$v` that
    // releases the reference it reads, letting the update happen in place.
    // Without that, jaq copies the whole container on every path, so
    // `.[] |= f` became quadratic — 2.4 s for 10,000 elements against jaq's
    // 17 ms.
    Def {
        name: "_pf_vivify",
        deps: &["_pf_toint"],
        src: Some(
            "def _pf_vivify_slow($p): \
             ((first(range($p|length) as $i | select(($p[$i]|type) as $t \
             | $t != \"number\" and $t != \"string\") | $i)) // ($p|length)) as $lim \
             | reduce range(0; [$lim + 1, ($p|length)] | min) as $i (.; \
             ($p[$i]) as $k | ($k|type) as $kt | getpath($p[:$i]) as $c | ($c|type) as $ct \
             | if $kt == \"number\" and ($ct == \"null\" or $ct == \"array\") then \
             (if $ct == \"null\" then [] else $c end) as $a | ($k|_pf_toint) as $ki \
             | if $ki < 0 and ($ki + ($a|length)) < 0 then error(\"Out of bounds negative array index\") \
             elif $ki > 536870911 then error(\"Array index too large\") \
             elif $ct == \"null\" or $ki >= ($a|length) \
             then setpath($p[:$i]; $a + [range($a|length; [$ki + 1, ($a|length)] | max) | null]) \
             else . end \
             elif $ct == \"null\" then \
             (if $kt == \"string\" then setpath($p[:$i]; {}) elif $kt == \"object\" then setpath($p[:$i]; []) \
             else error(\"Cannot index null with \\($kt)\") end) \
             elif $ct == \"object\" then (if $kt == \"string\" then . else error(\"Cannot index object with \\($kt)\") end) \
             elif $ct == \"array\" then (if $kt == \"object\" then . else error(\"Cannot update field at \\($kt) index of array\") end) \
             elif $ct == \"string\" and $kt == \"object\" then error(\"Cannot update string slices\") \
             else error(\"Cannot index \\($ct) with \\($kt)\") end); \
             def _pf_vivify(paths): reduce path(paths) as $p (.; \
             if ($p|length) > 0 and (try (($p[-1]) as $k | getpath($p[:-1]) \
             | (type == \"object\" and ($k|type) == \"string\") \
             or (type == \"array\" and ($k|type) == \"number\" and $k == ($k|floor) \
             and $k < length and $k + length >= 0)) catch false) \
             then . else _pf_vivify_slow($p) end);",
        ),
    },
    // jq's assignment builtins, which the parser-based rewrite in
    // `syntax::rewrite` calls in place of the `=` / `|=` / `op=` operators.
    // jq 1.8.1's `_assign` and `_modify`, verbatim except: `setpath` and
    // `delpaths` become `_pf_setpath` (auto-vivifying, which is the point) and
    // `_pf_delpaths` (jq's deletion order, so `|= empty` deletes correctly),
    // and jq's builtin-private `$$$$v` spelling (a variable read that also
    // releases it) becomes an ordinary variable with the same value.
    Def {
        name: "_pf_assign",
        deps: &["_pf_setpath"],
        src: Some(
            "def _pf_assign(paths; $value): reduce path(paths) as $p (.; _pf_setpath($p; $value));",
        ),
    },
    Def {
        name: "_pf_modify",
        deps: &["_pf_setpath", "_pf_delpaths"],
        src: Some(
            "def _pf_modify(paths; update): reduce path(paths) as $p ([., []]; \
             . as $__pf_dot | null | label $__pf_out | ($__pf_dot[0] | getpath($p)) as $__pf_v \
             | (($__pf_v | update | (., break $__pf_out) as $__pf_v | $__pf_dot \
             | _pf_setpath([0] + $p; $__pf_v)), \
             ($__pf_dot | _pf_setpath([1, (.[1] | length)]; $p)))) \
             | . as $__pf_dot | $__pf_dot[0] | _pf_delpaths($__pf_dot[1]);",
        ),
    },
    // The literal regexes of this filter with their groups named, built per
    // run by `dynamic` from `src/regex.rs`.
    Def {
        name: "_pf_regex_lit",
        deps: &[],
        src: None,
    },
    // jq 1.8.1's `jv_dump_string_trunc` with its 15-byte buffer, which every
    // `type (value)` error message uses: the JSON text, cut to 11 bytes plus
    // `...` when it does not fit in 14 — backing off to a character boundary
    // rather than splitting UTF-8 (`"☆☆☆...`, not 11 bytes of it).
    Def {
        name: "_pf_dump",
        deps: &[],
        src: Some(
            "def _pf_dump: tojson | if utf8bytelength <= 14 then . else explode as $__pf_cs \
             | ([foreach $__pf_cs[] as $c (0; . + (if $c < 128 then 1 elif $c < 2048 then 2 \
             elif $c < 65536 then 3 else 4 end)) | select(. <= 11)] | length) as $__pf_n \
             | ($__pf_cs[:$__pf_n] | implode) + \"...\" end;",
        ),
    },
    // jq's `@base64d` decodes up to the first `=` and ignores the rest,
    // accepts missing padding, and rejects only a character outside the
    // standard alphabet or a single leftover character. jaq's requires exact
    // padding, and its errors differ. jq also ignores the unused low bits of
    // the last character (`"QR"` is `"A"`), which jaq rejects; they are
    // cleared before decoding. jaq runs first; what it rejects is
    // re-decoded the way jq reads it, with jq's errors. Both read `tostring`.
    Def {
        name: "_pf_base64d",
        deps: &["_pf_dump"],
        src: Some(
            "def _pf_base64d: tostring | . as $__pf_s | try @base64d catch ($__pf_s \
             | (split(\"=\")[0]) as $__pf_t \
             | if ($__pf_t | test(\"^[A-Za-z0-9+/]*$\") | not) \
             then error(\"string (\\($__pf_s|_pf_dump)) is not valid base64 data\") \
             elif ($__pf_t | length) % 4 == 1 \
             then error(\"string (\\($__pf_s|_pf_dump)) trailing base64 byte found\") \
             else (($__pf_t | length) % 4) as $__pf_r \
             | \"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/\" as $__pf_a \
             | (if $__pf_r == 0 then $__pf_t else ($__pf_a | index($__pf_t[-1:])) as $__pf_v \
             | $__pf_t[:-1] + $__pf_a[$__pf_v - $__pf_v % (if $__pf_r == 2 then 16 else 4 end):][:1] end) \
             + (\"==\" | .[:(4 - $__pf_r) % 4]) | @base64d end);",
        ),
    },
    // jq's `@urid` rejects a `%` not followed by two hex digits, and decoded
    // bytes that are not UTF-8; jaq keeps the first as text and replaces the
    // second with U+FFFD. Decoding added a U+FFFD the input did not spell —
    // literally or as `%EF%BF%BD` — exactly when the bytes were invalid.
    Def {
        name: "_pf_urid",
        deps: &["_pf_dump"],
        src: Some(
            "def _pf_urid: tostring | if index(\"%\") == null then . \
             else . as $__pf_s | if test(\"^([^%]|%[0-9A-Fa-f]{2})*$\") | not \
             then error(\"string (\\($__pf_s|_pf_dump)) is not a valid uri encoding\") \
             else @urid as $__pf_d \
             | if ([$__pf_d | match(\"\\uFFFD\"; \"g\")] | length) \
             > ([$__pf_s | match(\"\\uFFFD|%[Ee][Ff]%[Bb][Ff]%[Bb][Dd]\"; \"g\")] | length) \
             then error(\"string (\\($__pf_s|_pf_dump)) is not a valid uri encoding\") \
             else $__pf_d end end end;",
        ),
    },
    // jq's wording for the errors jaq raises with its own, applied to what a
    // `catch` handler receives. Each of jaq's templates carries the values
    // involved as JSON, so they are parsed back out and re-described the way
    // jq's `type_error` and `jv_get` describe them. Anything else, and any
    // non-string error value, passes through unchanged. Only the error path
    // pays for it.
    Def {
        name: "_pf_err",
        deps: &["_pf_dump"],
        src: Some(concat!(
            r#"def _pf_err1: [try fromjson catch empty] | if length == 1 then .[0] else empty end; "#,
            r#"def _pf_err: if type != "string" then . "#,
            r#"elif (startswith("cannot ") or endswith(" has no length")) | not then . else . as $m | first("#,
            r#"(if startswith("cannot use ") and endswith(" as iterable (array or object)") "#,
            r#"then $m[11:-30] | _pf_err1 | "Cannot iterate over \(type) (\(_pf_dump))" else empty end), "#,
            r#"(if startswith("cannot use ") and endswith(" as number") "#,
            r#"then $m[11:-10] | _pf_err1 | "\(type) (\(_pf_dump)) number required" else empty end), "#,
            r#"(if startswith("cannot use ") and endswith(" as rangeable (array or string)") "#,
            r#"then $m[11:-31] | _pf_err1 | "Cannot index \(type) with object" else empty end), "#,
            r#"(if endswith(" has no length") "#,
            r#"then $m[:-14] | _pf_err1 | "\(type) (\(_pf_dump)) has no length" else empty end), "#,
            // jq's `jv_get`: a short string key is spelled out, raw.
            r#"(if startswith("cannot index ") then $m[13:] | indices(" with ")[] as $i "#,
            r#"| (.[:$i] | _pf_err1) as $v | (.[$i + 6:] | _pf_err1) as $k "#,
            r#"| if ($k | type) == "string" and ($k | utf8bytelength) < 30 "#,
            r#"then "Cannot index \($v | type) with string \"" + $k + "\"" "#,
            r#"else "Cannot index \($v | type) with \($k | type)" end else empty end), "#,
            r#"(if startswith("cannot calculate ") then $m[17:] | (" + ", " - ", " * ", " / ", " % ") as $op "#,
            r#"| indices($op)[] as $i | (.[:$i] | _pf_err1) as $a | (.[$i + 3:] | _pf_err1) as $b "#,
            r#"| (($a | type) == "number" and ($b | type) == "number" and $b > -1 and $b < 1) as $zero "#,
            r#"| {"+": "cannot be added", "-": "cannot be subtracted", "*": "cannot be multiplied", "#,
            r#""/": "cannot be divided", "%": "cannot be divided (remainder)"}[$op[1:2]] "#,
            r#"+ (if $zero and ($op == " / " or $op == " % ") then " because the divisor is zero" else "" end) "#,
            r#"| "\($a | type) (\($a | _pf_dump)) and \($b | type) (\($b | _pf_dump)) \(.)" else empty end), "#,
            r#"$m) end;"#,
        )),
    },
    // jq's unary minus raises `cannot be negated`; jaq's says `cannot use …
    // as number`, which `catch` would translate to the math functions'
    // wording. The rewriter sends a non-literal negation here.
    Def {
        name: "_pf_neg",
        deps: &["_pf_dump"],
        src: Some(
            "def _pf_neg: . as $__pf_v | try (- $__pf_v) \
             catch ($__pf_v | error(\"\\(type) (\\(_pf_dump)) cannot be negated\"));",
        ),
    },
    // Pad a short broken-down-time array to jq's eight fields with zeros.
    Def {
        name: "_pf_pad_tm",
        deps: &[],
        src: Some(
            "def _pf_pad_tm: if type == \"array\" and length < 8 \
             then . + [range(8 - length) | 0] else . end;",
        ),
    },
];

/// What the prelude needs to know about the invocation.
#[derive(Debug, Default, Clone)]
pub struct Context {
    /// The single input file's name, when there is exactly one.
    ///
    /// jq's `input_filename` is `null` for stdin and the current file otherwise.
    /// With several inputs the answer changes as jq advances through them, which
    /// jaq exposes no way to track — see [`Context::ambiguous_filename`].
    pub input_filename: Option<String>,
    /// Set when more than one input file was given, making `input_filename`
    /// unanswerable rather than merely absent.
    pub ambiguous_filename: bool,
    /// The filter's plain string literals that contain `(`: candidate regexes
    /// whose groups are named ahead of time (see `crate::regex`).
    pub regex_literals: Vec<String>,
}

fn find<'a>(name: &str, table: &'a [Def]) -> Option<&'a Def> {
    table.iter().find(|d| d.name == name)
}

fn lookup(name: &str) -> Option<&'static Def> {
    find(name, POLYFILLS)
        .or_else(|| find(name, REPAIRS))
        .or_else(|| find(name, INTERNAL))
}

/// jq source for a definition that has to be built at run time.
fn dynamic(name: &str, ctx: &Context) -> String {
    match name {
        "_pf_setpath" => format!("def _pf_setpath($p; $v): {SETPATH_BODY};"),
        "builtins" => format!("def builtins: {};", JQ_BUILTINS.trim()),
        // Bound once rather than defined: a definition would rebuild the
        // table on every call (5 µs a call, measured).
        "_pf_regex_lit" => format!("{} as $__pf_rl |", regex_table(&ctx.regex_literals)),
        "input_filename" => match (&ctx.input_filename, ctx.ambiguous_filename) {
            // jq reports the file currently being read; with one input that is
            // a constant, so the polyfill is exact.
            (Some(path), false) => format!("def input_filename: {};", json_string(path)),
            // Reading stdin: jq answers null.
            (None, false) => "def input_filename: null;".to_owned(),
            // Several inputs: any constant would be wrong for some of them, so
            // fail loudly rather than answer quietly and wrongly.
            (_, true) => "def input_filename: error(\"input_filename with multiple input files is not supported by Pathfinder\");".to_owned(),
        },
        other => unreachable!("no dynamic source for {other}"),
    }
}

/// The `_pf_regex_lit` table: each literal regex with what the prelude may
/// assume about it ([`crate::regex::shape`]) and, where a group can go
/// unmatched, its groups named. A jq object keyed by the regex.
fn regex_table(literals: &[String]) -> String {
    let mut out = String::from("{");
    for re in literals {
        let shape = crate::regex::shape(re);
        let named = crate::regex::name_groups(re);
        let unnamed = named
            .as_ref()
            .is_some_and(|n| n.groups.iter().any(|(name, _)| name.is_none()));
        let (regex, groups) = match named {
            Some(n) if !shape.simple => (n.regex, n.groups),
            _ => (re.clone(), Vec::new()),
        };
        if out.len() > 1 {
            out.push_str(", ");
        }
        let name = |n: &Option<String>| n.as_deref().map_or_else(|| "null".to_owned(), json_string);
        let list: Vec<String> = groups
            .iter()
            .map(|(n, syn)| format!("[{}, {}]", name(n), json_string(syn)))
            .collect();
        let dict: Vec<String> = groups
            .iter()
            .map(|(n, syn)| format!("{}: {}", json_string(syn), name(n)))
            .collect();
        let _ = write!(
            out,
            "{}: {{re: {}, simple: {}, unnamed: {unnamed}, nullable: {}, groups: [{}], dict: {{{}}}}}",
            json_string(re),
            json_string(&regex),
            shape.simple,
            shape.nullable,
            list.join(", "),
            dict.join(", ")
        );
    }
    out.push('}');
    out
}

/// Encode a Rust string as a JSON string literal.
fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Names that are missing from jaq but cannot be expressed in the jq language.
///
/// Returned so the caller can emit one precise diagnostic naming the construct,
/// rather than letting jaq answer the less helpful `undefined filter`.
pub fn inexpressible(name: &str) -> Option<&'static str> {
    match name {
        "input_line_number" => {
            Some("jaq tracks no input position, so there is no line number to report")
        }
        "modulemeta" | "get_search_list" | "get_jq_origin" | "get_prog_origin" => {
            Some("jaq's module system exposes no introspection surface")
        }
        "lgamma_r" => Some("jaq provides no lgamma_r; use `lgamma` if an approximation will do"),
        _ => None,
    }
}

/// Render the definitions a filter needs, as a single line.
///
/// One line matters: jaq reports errors against the program text it was given,
/// so keeping the prelude on line 1 alongside the filter's own first line leaves
/// the user's line numbers unchanged rather than shifting them.
///
/// Returns an empty string when nothing is needed — the common case, and the one
/// that keeps the fast path byte-identical to what the user typed.
pub fn render(wanted: &[String], ctx: &Context) -> String {
    let mut chosen: Vec<&'static str> = Vec::new();
    for name in wanted {
        // Internal helpers are wanted by name too: the syntax rewriter asks for
        // `_pf_assign`/`_pf_modify` directly.
        if let Some(def) = lookup(name) {
            push_with_deps(def, &mut chosen);
        }
    }
    let mut out = String::new();
    for name in chosen {
        let def = lookup(name).expect("chosen names come from the tables");
        let src = match def.src {
            Some(s) => s.to_owned(),
            None => dynamic(def.name, ctx),
        };
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&src);
    }
    out
}

/// Add a definition after its dependencies, without duplicating either.
fn push_with_deps(def: &'static Def, chosen: &mut Vec<&'static str>) {
    if chosen.contains(&def.name) {
        return;
    }
    for dep in def.deps {
        if let Some(d) = lookup(dep) {
            push_with_deps(d, chosen);
        }
    }
    if !chosen.contains(&def.name) {
        chosen.push(def.name);
    }
}

/// Every name that triggers an injection, for `--explain` and for tests.
pub fn known_names() -> Vec<&'static str> {
    POLYFILLS.iter().chain(REPAIRS).map(|d| d.name).collect()
}

#[cfg(test)]
mod tests {
    use super::{Context, inexpressible, json_string, known_names, render};

    fn want(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn nothing_wanted_renders_nothing() {
        // The common case: an untouched program keeps the exec fast path.
        assert_eq!(render(&want(&["map", "select"]), &Context::default()), "");
    }

    #[test]
    fn renders_on_a_single_line() {
        let out = render(&want(&["tostream", "IN"]), &Context::default());
        assert!(
            !out.contains('\n'),
            "prelude must not shift line numbers: {out}"
        );
    }

    #[test]
    fn dependencies_come_first() {
        let out = render(&want(&["fromstream"]), &Context::default());
        let helper = out.find("def _pf_setpath").expect("helper injected");
        let user = out.find("def fromstream").expect("polyfill injected");
        assert!(helper < user, "helper must precede its dependant");
    }

    #[test]
    fn a_dependency_is_not_duplicated() {
        let out = render(&want(&["fromstream", "setpath"]), &Context::default());
        assert_eq!(out.matches("def _pf_setpath").count(), 1);
    }

    #[test]
    fn input_filename_follows_the_input_shape() {
        let stdin = render(&want(&["input_filename"]), &Context::default());
        assert!(stdin.contains("def input_filename: null;"));

        let one = render(
            &want(&["input_filename"]),
            &Context {
                input_filename: Some("a.json".to_owned()),
                ambiguous_filename: false,
                regex_literals: Vec::new(),
            },
        );
        assert!(one.contains(r#"def input_filename: "a.json";"#));

        let many = render(
            &want(&["input_filename"]),
            &Context {
                input_filename: None,
                ambiguous_filename: true,
                regex_literals: Vec::new(),
            },
        );
        // Wrong-but-quiet is the one thing this must not do.
        assert!(many.contains("error("));
    }

    #[test]
    fn removed_jq_names_are_not_polyfilled() {
        // None of these are jq 1.8.1 builtins; defining them would let a filter
        // run here and fail under real jq.
        for name in [
            "leaf_paths",
            "ascii",
            "isvalid",
            "toarray",
            "ANY",
            "ALL",
            "GROUP_BY",
            "UNIQUE_BY",
        ] {
            assert!(
                !known_names().contains(&name),
                "{name} must not be polyfilled"
            );
            assert_eq!(render(&want(&[name]), &Context::default()), "");
        }
    }

    #[test]
    fn repairs_never_call_the_builtin_they_shadow() {
        // Self-reference here is infinite recursion under jaq, not a super-call.
        let out = render(&want(&["setpath", "todate"]), &Context::default());
        assert!(!out.contains("def setpath($p; $v): setpath("));
        assert!(!out.contains("def todate: todate;"));
    }

    #[test]
    fn aliases_precede_the_shadows_that_use_them() {
        // An alias defined after its shadow would bind the shadow itself and
        // recurse forever; before it, the alias reaches jaq's builtin.
        for (alias, shadow) in [
            ("def _pf_mktime:", "def mktime:"),
            ("def _pf_strftime(", "def strftime("),
            ("def _pf_match01(", "def match($val)"),
            ("def _pf_startswith(", "def startswith("),
        ] {
            let name = shadow
                .trim_start_matches("def ")
                .split([':', '('])
                .next()
                .unwrap_or_default();
            let out = render(&want(&[name]), &Context::default());
            let a = out
                .find(alias)
                .unwrap_or_else(|| panic!("{alias} missing: {out}"));
            let s = out
                .find(shadow)
                .unwrap_or_else(|| panic!("{shadow} missing: {out}"));
            assert!(a < s, "{alias} must precede {shadow}");
        }
    }

    #[test]
    fn todate_binds_the_repaired_strftime() {
        // `[2024,2,15] | todate` works in jq only because jq's todate goes
        // through a strftime that accepts short arrays; ours must too, which
        // means the strftime shadow is emitted before todateiso8601.
        let out = render(&want(&["todate"]), &Context::default());
        let shadow = out.find("def strftime(").expect("strftime repair injected");
        let todate = out
            .find("def todateiso8601:")
            .expect("todateiso8601 injected");
        assert!(shadow < todate);
    }

    #[test]
    fn setpath_guards_jq_array_index_errors() {
        let out = render(&want(&["setpath"]), &Context::default());
        assert!(out.contains("Out of bounds negative array index"));
        assert!(out.contains("Array index too large"));
        assert!(
            out.contains("536870911"),
            "must use jq's INT_MAX >> 2 limit"
        );
    }

    #[test]
    fn regex_one_argument_forms_never_pass_null_flags_to_jaq() {
        // jaq's two-argument regex builtins reject null flags; jq accepts them.
        // Only the repaired `match` may receive `null`; it routes that case to
        // jaq's one-argument builtin.
        for name in ["match", "test", "capture", "sub", "scan"] {
            let out = render(&want(&[name]), &Context::default());
            for builtin in ["_pf_match0(", "_pf_test(", "_pf_capture("] {
                for (at, _) in out.match_indices(builtin) {
                    let call = &out[at..out[at..].find(')').map_or(out.len(), |e| at + e)];
                    assert!(!call.contains("null"), "{name} passes null flags: {call}");
                }
            }
            if out.contains("def _pf_raw(") {
                assert!(out.contains("if $mode == null then _pf_match01($re)"));
            }
        }
    }

    #[test]
    fn literal_regexes_are_bound_once_ahead_of_the_regex_repairs() {
        let ctx = Context {
            regex_literals: vec!["(a)?b".to_owned(), "x+".to_owned()],
            ..Context::default()
        };
        let out = render(&want(&["match"]), &ctx);
        let table = out.find("as $__pf_rl |").expect("table bound");
        assert!(table < out.find("def _pf_regex(").expect("lookup defined"));
        // A skippable group is renamed; a regex without groups is not.
        assert!(out.contains(r#""(a)?b": {re: "(?<__pf_g0>a)?b", simple: false"#));
        assert!(out.contains(r#""x+": {re: "x+", simple: true, unnamed: false, nullable: false"#));
    }

    #[test]
    fn inexpressible_names_are_named_not_polyfilled() {
        assert!(inexpressible("input_line_number").is_some());
        assert!(inexpressible("modulemeta").is_some());
        assert!(inexpressible("tostream").is_none());
    }

    #[test]
    fn json_string_escapes_what_it_must() {
        assert_eq!(json_string("a\"b"), "\"a\\\"b\"");
        assert_eq!(json_string("a\\b"), "\"a\\\\b\"");
        assert_eq!(json_string("a\nb"), "\"a\\nb\"");
        assert_eq!(json_string("\u{1}"), "\"\\u0001\"");
    }
}
