// SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Reprint a parsed program with every grouping made explicit.
//!
//! This exists to test the parser, not to run programs: if the tree groups an
//! expression differently from jq, the fully parenthesised text means something
//! different, and running it changes the output. Routing the whole conformance
//! suite through this printer (`PATHFINDER_DEBUG_REPRINT=1`) therefore checks
//! every precedence decision against jq's own tests.
//!
//! It also normalises the few spellings jaq handles differently from jq, so the
//! printed form is valid for both: shorthand object entries are expanded,
//! keyword keys are quoted, and `if` without `else` gets an explicit `else .`.

use std::fmt::Write as _;

use super::{
    FieldName, Kind, Node, ObjKey, ObjPair, ObjPat, ObjShort, Param, Pattern, StrLit, StrSeg,
};

/// Print `node` (parsed from `src`) fully parenthesised.
pub fn full(node: &Node, src: &str) -> String {
    let mut out = String::new();
    p(node, src, &mut out);
    out
}

#[expect(
    clippy::too_many_lines,
    reason = "one arm per node kind, each a few lines; a split would be arbitrary"
)]
fn p(n: &Node, src: &str, o: &mut String) {
    match &n.kind {
        Kind::Pipe(a, b) => bin(a, "|", b, src, o),
        Kind::Comma(a, b) => bin(a, ",", b, src, o),
        Kind::Binary { op, lhs, rhs } => bin(lhs, op, rhs, src, o),
        Kind::Bind {
            source,
            patterns,
            body,
        } => {
            o.push_str("((");
            p(source, src, o);
            o.push_str(") as ");
            pats(patterns, src, o);
            o.push_str(" | ");
            p(body, src, o);
            o.push(')');
        }
        Kind::Label { name, body } => {
            let _ = write!(o, "(label ${name} | ");
            p(body, src, o);
            o.push(')');
        }
        Kind::Def { def, rest } => {
            let _ = write!(o, "(def {}", def.name);
            if !def.params.is_empty() {
                let ps: Vec<String> = def
                    .params
                    .iter()
                    .map(|p| match p {
                        Param::Filter(n) => n.clone(),
                        Param::Value(n) => format!("${n}"),
                    })
                    .collect();
                let _ = write!(o, "({})", ps.join("; "));
            }
            o.push_str(": ");
            p(&def.body, src, o);
            o.push_str("; ");
            p(rest, src, o);
            o.push(')');
        }
        Kind::Neg(t) => {
            o.push_str("(-(");
            p(t, src, o);
            o.push_str("))");
        }
        Kind::Identity => o.push('.'),
        Kind::Recurse => o.push_str(".."),
        Kind::Number => o.push_str(&src[n.span.start..n.span.end]),
        Kind::Str(s) => string(s, src, o),
        Kind::Format(f) => {
            let _ = write!(o, "@{f}");
        }
        Kind::Var(v) => {
            let _ = write!(o, "${v}");
        }
        Kind::Loc => o.push_str("$__loc__"),
        Kind::Break(l) => {
            let _ = write!(o, "break ${l}");
        }
        Kind::Call { name, args } => {
            o.push_str(name);
            if !args.is_empty() {
                o.push('(');
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        o.push_str("; ");
                    }
                    p(a, src, o);
                }
                o.push(')');
            }
        }
        Kind::Paren(q) => {
            o.push('(');
            p(q, src, o);
            o.push(')');
        }
        Kind::Array(q) => {
            o.push('[');
            if let Some(q) = q {
                p(q, src, o);
            }
            o.push(']');
        }
        Kind::Object(pairs) => object(pairs, src, o),
        Kind::Reduce {
            source,
            patterns,
            init,
            update,
        } => {
            o.push_str("(reduce (");
            p(source, src, o);
            o.push_str(") as ");
            pats(patterns, src, o);
            o.push_str(" ((");
            p(init, src, o);
            o.push_str("); (");
            p(update, src, o);
            o.push_str(")))");
        }
        Kind::Foreach {
            source,
            patterns,
            init,
            update,
            extract,
        } => {
            o.push_str("(foreach (");
            p(source, src, o);
            o.push_str(") as ");
            pats(patterns, src, o);
            o.push_str(" ((");
            p(init, src, o);
            o.push_str("); (");
            p(update, src, o);
            if let Some(x) = extract {
                o.push_str("); (");
                p(x, src, o);
            }
            o.push_str(")))");
        }
        Kind::If {
            branches,
            otherwise,
        } => {
            o.push('(');
            for (i, (c, t)) in branches.iter().enumerate() {
                o.push_str(if i == 0 { "if (" } else { " elif (" });
                p(c, src, o);
                o.push_str(") then (");
                p(t, src, o);
                o.push(')');
            }
            o.push_str(" else (");
            match otherwise {
                Some(e) => p(e, src, o),
                None => o.push('.'),
            }
            o.push_str(") end)");
        }
        Kind::Try { body, handler } => {
            o.push_str("(try (");
            p(body, src, o);
            o.push(')');
            if let Some(h) = handler {
                o.push_str(" catch (");
                p(h, src, o);
                o.push(')');
            }
            o.push(')');
        }
        Kind::Field { base, name } => {
            postfix_base(base, src, o);
            match name {
                FieldName::Ident(f) => {
                    let _ = write!(o, ".[\"{f}\"]");
                }
                FieldName::Str(s) => {
                    o.push_str(".[");
                    string(s, src, o);
                    o.push(']');
                }
            }
        }
        Kind::Index { base, index } => {
            postfix_base(base, src, o);
            o.push_str(".[");
            p(index, src, o);
            o.push(']');
        }
        Kind::Slice { base, from, to } => {
            postfix_base(base, src, o);
            o.push_str(".[");
            if let Some(f) = from {
                p(f, src, o);
            }
            o.push(':');
            if let Some(t) = to {
                p(t, src, o);
            }
            o.push(']');
        }
        Kind::Iterate(base) => {
            postfix_base(base, src, o);
            o.push_str(".[]");
        }
        Kind::Optional(t) => {
            o.push('(');
            p(t, src, o);
            o.push_str(")?");
        }
    }
}

/// The base of a postfix chain. An implicit `.` prints as nothing so `.foo`
/// becomes `.["foo"]` rather than `..["foo"]`; anything else is parenthesised.
fn postfix_base(base: &Node, src: &str, o: &mut String) {
    if matches!(base.kind, Kind::Identity) {
        return;
    }
    o.push('(');
    p(base, src, o);
    o.push(')');
}

fn bin(a: &Node, op: &str, b: &Node, src: &str, o: &mut String) {
    o.push_str("((");
    p(a, src, o);
    let _ = write!(o, ") {op} (");
    p(b, src, o);
    o.push_str("))");
}

fn string(s: &StrLit, src: &str, o: &mut String) {
    if let Some(f) = &s.format {
        let _ = write!(o, "@{f} ");
    }
    o.push('"');
    for seg in &s.parts {
        match seg {
            StrSeg::Text(span) => o.push_str(&src[span.start..span.end]),
            StrSeg::Interp(q) => {
                o.push_str("\\(");
                p(q, src, o);
                o.push(')');
            }
        }
    }
    o.push('"');
}

/// A JSON string literal for a plain name.
fn quoted(name: &str) -> String {
    let mut s = String::with_capacity(name.len() + 2);
    s.push('"');
    for c in name.chars() {
        if c == '"' || c == '\\' {
            s.push('\\');
        }
        s.push(c);
    }
    s.push('"');
    s
}

fn key(k: &ObjKey, src: &str, o: &mut String) {
    match k {
        ObjKey::Ident(name) => o.push_str(&quoted(name)),
        ObjKey::Str(s) => string(s, src, o),
        ObjKey::Var(v) => {
            let _ = write!(o, "(${v})");
        }
        ObjKey::Expr(q) => p(q, src, o),
    }
}

fn object(pairs: &[ObjPair], src: &str, o: &mut String) {
    o.push('{');
    for (i, pair) in pairs.iter().enumerate() {
        if i > 0 {
            o.push_str(", ");
        }
        match pair {
            ObjPair::KeyValue { key: k, value } => {
                key(k, src, o);
                o.push_str(": (");
                p(value, src, o);
                o.push(')');
            }
            ObjPair::Shorthand(ObjShort::Ident(name)) => {
                let q = quoted(name);
                let _ = write!(o, "{q}: .[{q}]");
            }
            ObjPair::Shorthand(ObjShort::Var(v)) => {
                let _ = write!(o, "{}: ${v}", quoted(v));
            }
            ObjPair::Shorthand(ObjShort::Str(s)) => {
                // `{"k"}` is `{"k": .["k"]}`; an interpolated key would be
                // evaluated twice by that expansion, so keep it as written.
                if s.format.is_none() && s.parts.iter().all(|seg| matches!(seg, StrSeg::Text(_))) {
                    string(s, src, o);
                    o.push_str(": .[");
                    string(s, src, o);
                    o.push(']');
                } else {
                    string(s, src, o);
                }
            }
            ObjPair::Shorthand(ObjShort::Loc(_)) => o.push_str("$__loc__"),
        }
    }
    o.push('}');
}

fn pats(ps: &[Pattern], src: &str, o: &mut String) {
    for (i, pat) in ps.iter().enumerate() {
        if i > 0 {
            o.push_str(" ?// ");
        }
        pattern(pat, src, o);
    }
}

/// Print one destructuring pattern. `{$b: p}` is spelled `{"b": $b, "b": p}`,
/// which jaq accepts and means the same: bind `$b` to `.b`, then destructure it.
pub fn pattern(pat: &Pattern, src: &str, o: &mut String) {
    match pat {
        Pattern::Var(v) => {
            let _ = write!(o, "${v}");
        }
        Pattern::Array(items) => {
            o.push('[');
            for (i, it) in items.iter().enumerate() {
                if i > 0 {
                    o.push_str(", ");
                }
                pattern(it, src, o);
            }
            o.push(']');
        }
        Pattern::Object(items) => {
            o.push('{');
            for (i, it) in items.iter().enumerate() {
                if i > 0 {
                    o.push_str(", ");
                }
                match it {
                    ObjPat::Var(v) => {
                        let _ = write!(o, "${v}");
                    }
                    ObjPat::VarPattern(v, p2) => {
                        let _ = write!(o, "{q}: ${v}, {q}: ", q = quoted(v));
                        pattern(p2, src, o);
                    }
                    ObjPat::Key(k, p2) => {
                        key(k, src, o);
                        o.push_str(": ");
                        pattern(p2, src, o);
                    }
                }
            }
            o.push('}');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::full;
    use crate::syntax::parse::parse;

    fn reprint(src: &str) -> String {
        full(&parse(src).expect("parses"), src)
    }

    #[test]
    fn grouping_is_made_explicit() {
        assert_eq!(reprint("1 + 2 * 3"), "((1) + (((2) * (3))))");
        assert_eq!(reprint(".a = 1 // 2"), "((((.[\"a\"]) = (1))) // (2))");
    }

    #[test]
    fn shorthand_objects_are_expanded() {
        assert_eq!(reprint("{a, $b}"), "{\"a\": .[\"a\"], \"b\": $b}");
    }

    #[test]
    fn if_without_else_gets_an_explicit_identity() {
        assert_eq!(reprint("if . then 1 end"), "(if (.) then (1) else (.) end)");
    }

    #[test]
    fn printed_programs_parse_again() {
        // The printer's output must itself be valid jq. (It is not a fixpoint:
        // reprinting adds another layer of harmless parentheses.)
        for src in [
            ".a.b[0]?",
            "reduce .[] as $x (0; . + $x)",
            "\"a\\(.b)c\"",
            "def f($x): $x + 1; f(2)",
        ] {
            let once = reprint(src);
            assert!(parse(&once).is_ok(), "{src} -> {once}");
        }
    }
}
