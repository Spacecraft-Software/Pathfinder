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
/// Only `null` is widened. A path that indexes a *wrong-typed* existing value
/// still errors, matching jq — including the object-indexed-by-number case,
/// which jaq's own `.[$k] = v` would otherwise accept and turn into a
/// non-string object key.
///
/// It also reproduces jq's two array-index errors. A negative index reaching
/// before the start of the array is `Out of bounds negative array index`, and
/// an index above `INT_MAX >> 2` (536870911, the limit in jq's `jv_array_set`)
/// is `Array index too large`. Without the second guard, `.[999999999] = 0`
/// would pad a billion-element array instead of erroring.
const SETPATH_BODY: &str = concat!(
    "if ($p|length) == 0 then $v else ($p[0]) as $k ",
    "| (if . == null then (if ($k|type) == \"number\" then [] else {} end) else . end) as $c ",
    "| (if ($k|type) == \"number\" and ($c|type) == \"object\" ",
    "then error(\"Cannot index object with number\") else . end) ",
    "| (if ($k|type) == \"number\" and ($c|type) == \"array\" and $k < 0 and ($k + ($c|length)) < 0 ",
    "then error(\"Out of bounds negative array index\") else . end) ",
    "| (if ($k|type) == \"number\" and $k > 536870911 ",
    "then error(\"Array index too large\") else . end) ",
    "| (if ($k|type) == \"number\" and ($c|type) == \"array\" and $k >= ($c|length) ",
    "then $c + [range($c|length; $k+1) | null] else $c end) as $d ",
    "| $d | .[$k] = (($d|.[$k]) | _pf_setpath($p[1:]; $v)) end"
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
        deps: &[],
        src: Some(
            "def format($fmt): if $fmt == \"text\" then @text elif $fmt == \"json\" then @json \
             elif $fmt == \"csv\" then @csv elif $fmt == \"tsv\" then @tsv \
             elif $fmt == \"html\" then @html elif $fmt == \"uri\" then @uri \
             elif $fmt == \"sh\" then @sh elif $fmt == \"base64\" then @base64 \
             elif $fmt == \"base64d\" then @base64d \
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
    // --- regex ------------------------------------------------------------
    // jq's `scan` is always global: its definition matches with `"g" + $flags`.
    // jaq's `scan/1` returns only the first match (`"abcdc" | [scan("c")]` is
    // `["c"]`, not `["c","c"]`). jq 1.8.1's definition, verbatim; it goes
    // through `match`, which is not shadowed.
    Def {
        name: "scan",
        deps: &[],
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
    Def {
        name: "match",
        deps: &[],
        src: Some(
            "def _pf_match(re; mode): match(re; mode); def _pf_match1(re): match(re); def match($val): ($val|type) as $vt \
             | if $vt == \"string\" then _pf_match1($val) \
             elif $vt == \"array\" and ($val | length) > 1 then _pf_match($val[0]; $val[1]) \
             elif $vt == \"array\" and ($val | length) > 0 then _pf_match1($val[0]) \
             else error($vt + \" not a string or array\") end;",
        ),
    },
    Def {
        name: "test",
        deps: &[],
        src: Some(
            "def _pf_test(re; mode): test(re; mode); def _pf_test1(re): test(re); def test($val): ($val|type) as $vt \
             | if $vt == \"string\" then _pf_test1($val) \
             elif $vt == \"array\" and ($val | length) > 1 then _pf_test($val[0]; $val[1]) \
             elif $vt == \"array\" and ($val | length) > 0 then _pf_test1($val[0]) \
             else error($vt + \" not a string or array\") end;",
        ),
    },
    Def {
        name: "capture",
        deps: &[],
        src: Some(
            "def _pf_capture(re; mods): capture(re; mods); def _pf_capture1(re): capture(re); def capture($val): ($val|type) as $vt \
             | if $vt == \"string\" then _pf_capture1($val) \
             elif $vt == \"array\" and ($val | length) > 1 then _pf_capture($val[0]; $val[1]) \
             elif $vt == \"array\" and ($val | length) > 0 then _pf_capture1($val[0]) \
             else error($vt + \" not a string or array\") end;",
        ),
    },
    // --- dates --------------------------------------------------------------
    // jq accepts a broken-down time shorter than eight fields and treats the
    // missing fields as zero: `[2024,2,15] | mktime` is 1710460800. jaq
    // rejects it ("cannot convert [2024,2,15] to time"). Each repair pads, then
    // calls the builtin through an alias defined before the shadow.
    Def {
        name: "mktime",
        deps: &["_pf_pad_tm"],
        src: Some("def _pf_mktime: mktime; def mktime: _pf_pad_tm | _pf_mktime;"),
    },
    Def {
        name: "strftime",
        deps: &["_pf_pad_tm"],
        src: Some(
            "def _pf_strftime($f): strftime($f); def strftime($f): _pf_pad_tm | _pf_strftime($f);",
        ),
    },
    Def {
        name: "strflocaltime",
        deps: &["_pf_pad_tm"],
        src: Some(
            "def _pf_strflocaltime($f): strflocaltime($f); \
             def strflocaltime($f): _pf_pad_tm | _pf_strflocaltime($f);",
        ),
    },
];

/// Internal helpers, never triggered by name in a user filter.
static INTERNAL: &[Def] = &[
    Def {
        name: "_pf_setpath",
        deps: &[],
        src: None,
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
        if let Some(def) = find(name, POLYFILLS).or_else(|| find(name, REPAIRS)) {
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
            },
        );
        assert!(one.contains(r#"def input_filename: "a.json";"#));

        let many = render(
            &want(&["input_filename"]),
            &Context {
                input_filename: None,
                ambiguous_filename: true,
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
            ("def _pf_match1(", "def match($val)"),
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
        for name in ["match", "test", "capture"] {
            let out = render(&want(&[name]), &Context::default());
            assert!(!out.contains("; null)"), "{name} passes null flags: {out}");
        }
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
