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
//! - **Self-contained.** A repair must never call the builtin it shadows: under
//!   jaq that is infinite recursion, not a call to the original.
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
const SETPATH_BODY: &str = concat!(
    "if ($p|length) == 0 then $v else ($p[0]) as $k ",
    "| (if . == null then (if ($k|type) == \"number\" then [] else {} end) else . end) as $c ",
    "| (if ($k|type) == \"number\" and ($c|type) == \"object\" ",
    "then error(\"Cannot index object with number\") else . end) ",
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
    // jq truncates to whole seconds. Self-contained: `strftime` is native and
    // is not itself shadowed.
    Def {
        name: "todateiso8601",
        deps: &[],
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
];

/// Internal helpers, never triggered by name in a user filter.
static INTERNAL: &[Def] = &[Def {
    name: "_pf_setpath",
    deps: &[],
    src: None,
}];

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
