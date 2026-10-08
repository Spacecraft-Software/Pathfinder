// SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Programs jq refuses to compile, refused the same way.
//!
//! jaq accepts some programs jq rejects at compile time. Passing one through
//! would make a script work here and fail on the next machine — the worst
//! outcome for a shim — so Pathfinder reports jq's compile error and exits 3,
//! as jq does.
//!
//! The rejections mirror jq 1.8.1's own (measured, not recalled):
//!
//! - An object key, in a construction or a destructuring pattern, that jq's
//!   constant folding reduces to something other than a string:
//!   `{(0): 1}`, `{(1 + 1): 2}`, `. as {(true): $x}`. A key that is only known
//!   at run time is jq's run-time error, and jaq's, so it is left alone.
//! - `module` metadata that is not a constant object.
//! - `?//` anywhere but between destructuring patterns: jq's lexer reads
//!   `.a?//1` as `.a ?// 1`, a syntax error, where jaq reads `.a? // 1`.
//!
//! jq's folding is narrow, and so is this module's: literals, `$__loc__`,
//! arrays and objects of constants, parentheses, `c | .`, and arithmetic or
//! comparison between two constants that jq can evaluate. `and`, `or`, `//`,
//! negation, `as`, commas and calls are not folded by jq, so not here either.

use super::lex::{Kw, StrPart, Tok, Token, lex};
use super::parse::parse;
use super::{Kind, Node, ObjKey, ObjPair, ObjPat, Pattern, Span, StrLit, StrSeg};

/// A compile error, located in the program text it was found in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileError {
    pub message: String,
    pub span: Span,
}

impl CompileError {
    /// jq's rendering: the message with its location, the source line, and a
    /// caret under the offending text.
    pub fn render(&self, prog: &str, src: &str) -> String {
        let start = self.span.start.min(src.len());
        let line_start = src[..start].rfind('\n').map_or(0, |i| i + 1);
        let line_end = src[start..].find('\n').map_or(src.len(), |i| start + i);
        let line = src[..start].matches('\n').count() + 1;
        let column = src[line_start..start].chars().count() + 1;
        let width = src[start..self.span.end.clamp(start, line_end)]
            .chars()
            .count()
            .max(1);
        format!(
            "{prog}: error: {} at <top-level>, line {line}, column {column}:\n    {}\n    {}{}\n{prog}: 1 compile error\n",
            self.message,
            &src[line_start..line_end],
            " ".repeat(column - 1),
            "^".repeat(width),
        )
    }
}

/// The first compile error jq would report for `body`, if any. Spans are
/// relative to `body`. A body this parser cannot read yields `None`.
pub fn program(body: &str) -> Option<CompileError> {
    match parse(body) {
        Ok(tree) => first_error(&tree, body),
        // The parser accepts `?//` wherever jq's grammar does, so failing on
        // one is jq's syntax error. Any other failure may be a gap in this
        // parser rather than in the program, and is left to jaq.
        Err(e) if body.get(e.at..).is_some_and(|rest| rest.starts_with("?//")) => {
            Some(CompileError {
                message: "syntax error, unexpected ?//".to_owned(),
                span: Span::new(e.at, e.at + 3),
            })
        }
        Err(_) => None,
    }
}

/// The compile error jq reports for a `module` directive at the head of `src`.
pub fn module_header(src: &str) -> Option<CompileError> {
    let toks = lex(src).ok()?;
    if !matches!(toks.first().map(|t| &t.tok), Some(Tok::Keyword(Kw::Module))) {
        return None;
    }
    // The metadata runs to the first `;` at bracket depth zero.
    let mut depth = 0usize;
    let end = toks[1..].iter().find(|t| match &t.tok {
        Tok::Op("(" | "[" | "{") => {
            depth += 1;
            false
        }
        Tok::Op(")" | "]" | "}") => {
            depth = depth.saturating_sub(1);
            false
        }
        Tok::Op(";") => depth == 0,
        _ => false,
    })?;
    let start = toks.get(1)?.span.start;
    let span = Span::new(start, end.span.start);
    let text = &src[span.start..span.end];
    let message = match parse(text).ok().map(|n| constant(&n, text)) {
        Some(Some(Const::Obj(_))) => return None,
        Some(Some(_)) => "Module metadata must be an object",
        _ => "Module metadata must be constant",
    };
    Some(CompileError {
        message: message.to_owned(),
        span: Span::new(span.start, span.start + text.trim_end().len()),
    })
}

/// Whether the tokens contain `?//` at any depth.
pub fn has_alternative(toks: &[Token]) -> bool {
    toks.iter().any(|t| match &t.tok {
        Tok::Op("?//") => true,
        Tok::Str(parts) => parts
            .iter()
            .any(|p| matches!(p, StrPart::Interp(inner, _) if has_alternative(inner))),
        _ => false,
    })
}

/// Whether the tokens contain a computed object key — `(` right after `{` or
/// `,` — at any depth, string interpolations included. A cheap test for
/// whether a program is worth parsing for key checks.
pub fn has_computed_key(toks: &[Token]) -> bool {
    toks.windows(2)
        .any(|w| matches!((&w[0].tok, &w[1].tok), (Tok::Op("{" | ","), Tok::Op("("))))
        || toks.iter().any(|t| match &t.tok {
            Tok::Str(parts) => parts
                .iter()
                .any(|p| matches!(p, StrPart::Interp(inner, _) if has_computed_key(inner))),
            _ => false,
        })
}

/// Offsets of every `$__loc__` used as an object shorthand (`{$__loc__}`).
pub fn loc_shorthands(body: &str) -> Vec<usize> {
    fn walk(n: &Node, out: &mut Vec<usize>) {
        if let Kind::Object(pairs) = &n.kind {
            for p in pairs {
                if let ObjPair::Shorthand(super::ObjShort::Loc(span)) = p {
                    out.push(span.start);
                }
            }
        }
        for c in n.children() {
            walk(c, out);
        }
    }
    let mut out = Vec::new();
    if let Ok(tree) = parse(body) {
        walk(&tree, &mut out);
    }
    out
}

fn first_error(n: &Node, src: &str) -> Option<CompileError> {
    let here = match &n.kind {
        Kind::Object(pairs) => pairs.iter().find_map(|p| match p {
            ObjPair::KeyValue {
                key: ObjKey::Expr(k),
                ..
            } => key_error(k, src),
            _ => None,
        }),
        Kind::Bind { patterns, .. }
        | Kind::Reduce { patterns, .. }
        | Kind::Foreach { patterns, .. } => patterns.iter().find_map(|p| pattern_error(p, src)),
        _ => None,
    };
    here.or_else(|| n.children().into_iter().find_map(|c| first_error(c, src)))
}

fn pattern_error(p: &Pattern, src: &str) -> Option<CompileError> {
    match p {
        Pattern::Var(_) => None,
        Pattern::Array(items) => items.iter().find_map(|i| pattern_error(i, src)),
        Pattern::Object(items) => items.iter().find_map(|i| match i {
            ObjPat::Var(_) => None,
            ObjPat::Key(ObjKey::Expr(k), p) => key_error(k, src).or_else(|| pattern_error(p, src)),
            ObjPat::VarPattern(_, p) | ObjPat::Key(_, p) => pattern_error(p, src),
        }),
    }
}

fn key_error(k: &Node, src: &str) -> Option<CompileError> {
    let c = constant(k, src)?;
    if matches!(c, Const::Str(_)) {
        return None;
    }
    // jq points at the expression, not at the parentheses around it.
    let span = match &k.kind {
        Kind::Paren(inner) => inner.span,
        _ => k.span,
    };
    Some(CompileError {
        message: format!(
            "Cannot use {} ({}) as object key",
            c.type_name(),
            truncated(&c.dump())
        ),
        span,
    })
}

/// jq's `jv_dump_string_trunc` with its 15-byte buffer: longer dumps keep 11
/// bytes and gain `...`.
fn truncated(s: &str) -> String {
    if s.len() <= 14 {
        return s.to_owned();
    }
    let mut cut = 11;
    while !s.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}...", &s[..cut])
}

/// A value jq's constant folding would know at compile time.
#[derive(Debug, Clone, PartialEq)]
enum Const {
    Null,
    Bool(bool),
    Num(f64),
    /// The literal's text as written, escapes included.
    Str(String),
    Arr(Vec<Self>),
    Obj(Vec<(String, Self)>),
}

impl Const {
    const fn type_name(&self) -> &'static str {
        match self {
            Self::Null => "null",
            Self::Bool(_) => "boolean",
            Self::Num(_) => "number",
            Self::Str(_) => "string",
            Self::Arr(_) => "array",
            Self::Obj(_) => "object",
        }
    }

    const fn rank(&self) -> u8 {
        match self {
            Self::Null => 0,
            Self::Bool(false) => 1,
            Self::Bool(true) => 2,
            Self::Num(_) => 3,
            Self::Str(_) => 4,
            Self::Arr(_) => 5,
            Self::Obj(_) => 6,
        }
    }

    fn dump(&self) -> String {
        match self {
            Self::Null => "null".to_owned(),
            Self::Bool(b) => b.to_string(),
            Self::Num(n) => number(*n),
            Self::Str(s) => format!("\"{s}\""),
            Self::Arr(items) => {
                let inner: Vec<String> = items.iter().map(Self::dump).collect();
                format!("[{}]", inner.join(","))
            }
            Self::Obj(pairs) => {
                let inner: Vec<String> = pairs
                    .iter()
                    .map(|(k, v)| format!("\"{k}\":{}", v.dump()))
                    .collect();
                format!("{{{}}}", inner.join(","))
            }
        }
    }

    /// jq's ordering, for folding `<` and friends. Objects are not ordered
    /// here; a comparison involving one is left unfolded.
    fn order(&self, other: &Self) -> Option<std::cmp::Ordering> {
        use std::cmp::Ordering;
        match (self, other) {
            (Self::Num(a), Self::Num(b)) => a.partial_cmp(b),
            (Self::Str(a), Self::Str(b)) => Some(a.cmp(b)),
            (Self::Arr(a), Self::Arr(b)) => {
                for (x, y) in a.iter().zip(b) {
                    match x.order(y)? {
                        Ordering::Equal => {}
                        o => return Some(o),
                    }
                }
                Some(a.len().cmp(&b.len()))
            }
            (Self::Obj(_), Self::Obj(_)) => None,
            _ => Some(self.rank().cmp(&other.rank())),
        }
    }
}

/// A number as jq prints a computed one: integers without a fraction.
fn number(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e17 {
        format!("{n:.0}")
    } else {
        n.to_string()
    }
}

/// The constant `n` folds to under jq's rules, if it does.
fn constant(n: &Node, src: &str) -> Option<Const> {
    match &n.kind {
        Kind::Number => src[n.span.start..n.span.end].parse().ok().map(Const::Num),
        Kind::Str(s) => plain(s, src).map(Const::Str),
        Kind::Loc => Some(Const::Obj(vec![
            ("file".to_owned(), Const::Str("<top-level>".to_owned())),
            (
                "line".to_owned(),
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "a line number is far below 2^52"
                )]
                Const::Num((src[..n.span.start].matches('\n').count() + 1) as f64),
            ),
        ])),
        Kind::Call { name, args } if args.is_empty() => match name.as_str() {
            "null" => Some(Const::Null),
            "true" => Some(Const::Bool(true)),
            "false" => Some(Const::Bool(false)),
            _ => None,
        },
        Kind::Paren(q) => constant(q, src),
        Kind::Pipe(a, b) if matches!(b.kind, Kind::Identity) => constant(a, src),
        Kind::Array(None) => Some(Const::Arr(Vec::new())),
        Kind::Array(Some(q)) => {
            let mut items = Vec::new();
            comma_items(q, src, &mut items)?;
            Some(Const::Arr(items))
        }
        Kind::Object(pairs) => pairs
            .iter()
            .map(|p| match p {
                ObjPair::KeyValue { key, value } => {
                    let k = match key {
                        ObjKey::Ident(k) => k.clone(),
                        ObjKey::Str(s) => plain(s, src)?,
                        _ => return None,
                    };
                    Some((k, constant(value, src)?))
                }
                ObjPair::Shorthand(_) => None,
            })
            .collect::<Option<Vec<_>>>()
            .map(Const::Obj),
        Kind::Binary { op, lhs, rhs } => fold(op, constant(lhs, src)?, constant(rhs, src)?),
        _ => None,
    }
}

fn comma_items(q: &Node, src: &str, out: &mut Vec<Const>) -> Option<()> {
    if let Kind::Comma(a, b) = &q.kind {
        comma_items(a, src, out)?;
        comma_items(b, src, out)
    } else {
        out.push(constant(q, src)?);
        Some(())
    }
}

/// A string literal without interpolation, as written.
fn plain(s: &StrLit, src: &str) -> Option<String> {
    s.parts
        .iter()
        .map(|p| match p {
            StrSeg::Text(span) => Some(&src[span.start..span.end]),
            StrSeg::Interp(_) => None,
        })
        .collect()
}

/// One binary operator on two constants, where jq folds it. An operation jq
/// would reject at run time (`1 / 0`, `"a" + 1`) is not folded, by jq or here.
fn fold(op: &str, a: Const, b: Const) -> Option<Const> {
    use Const::{Arr, Bool, Null, Num, Obj, Str};
    use std::cmp::Ordering::{Greater, Less};
    Some(match (op, a, b) {
        ("==", a, b) => Bool(a == b),
        ("!=", a, b) => Bool(a != b),
        ("<", a, b) => Bool(a.order(&b)? == Less),
        ("<=", a, b) => Bool(a.order(&b)? != Greater),
        (">", a, b) => Bool(a.order(&b)? == Greater),
        (">=", a, b) => Bool(a.order(&b)? != Less),
        ("+", Null, x) | ("+", x, Null) => x,
        ("+", Num(x), Num(y)) => Num(x + y),
        ("+", Str(x), Str(y)) => Str(x + &y),
        ("+", Arr(mut x), Arr(y)) => {
            x.extend(y);
            Arr(x)
        }
        ("+", Obj(mut x), Obj(y)) => {
            for (k, v) in y {
                match x.iter_mut().find(|(xk, _)| *xk == k) {
                    Some(slot) => slot.1 = v,
                    None => x.push((k, v)),
                }
            }
            Obj(x)
        }
        ("-", Num(x), Num(y)) => Num(x - y),
        ("-", Arr(x), Arr(y)) => Arr(x.into_iter().filter(|e| !y.contains(e)).collect()),
        ("*", Num(x), Num(y)) => Num(x * y),
        ("/", Num(x), Num(y)) if y != 0.0 => Num(x / y),
        ("%", Num(x), Num(y)) => {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "jq's `%` truncates both operands to integers"
            )]
            let (x, y) = (x as i64, y as i64);
            // C's `%`, as jq uses it: the sign follows the dividend.
            #[expect(clippy::cast_precision_loss, reason = "the remainder is small")]
            Num(x.checked_rem(y)? as f64)
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::{module_header, program};

    fn rejects(src: &str) -> Option<String> {
        program(src).map(|e| e.message)
    }

    #[test]
    fn a_constant_non_string_key_is_a_compile_error() {
        assert_eq!(
            rejects("{(0):1}").as_deref(),
            Some("Cannot use number (0) as object key")
        );
        assert_eq!(
            rejects("{non_const:., (0):1}").as_deref(),
            Some("Cannot use number (0) as object key")
        );
        assert_eq!(
            rejects(". as {(true):$foo} | $foo").as_deref(),
            Some("Cannot use boolean (true) as object key")
        );
        assert_eq!(
            rejects("{(1+1):2}").as_deref(),
            Some("Cannot use number (2) as object key")
        );
        assert_eq!(
            rejects("{([1,2]):1}").as_deref(),
            Some("Cannot use array ([1,2]) as object key")
        );
        assert_eq!(
            rejects("{($__loc__):1}").as_deref(),
            Some("Cannot use object ({\"file\":\"<t...) as object key")
        );
    }

    #[test]
    fn keys_jq_only_knows_at_run_time_are_left_to_run_time() {
        for src in [
            "{(.a):1}",
            "{(-1):1}",
            "{(1,2):1}",
            "{(true and true):1}",
            "{(null // 1):1}",
            "{(1/0):1}",
            "{(\"a\"+1):1}",
            "{(\"a\"):1}",
            "{(\"a\" + \"b\"):1}",
            "{(1|tostring):1}",
        ] {
            assert_eq!(rejects(src), None, "{src}");
        }
    }

    #[test]
    fn the_error_points_where_jq_points() {
        let e = program("{non_const:., (0):1}").expect("rejected");
        let out = e.render("jq", "{non_const:., (0):1}");
        assert!(out.starts_with("jq: error: Cannot use number (0) as object key at <top-level>, line 1, column 16:\n"), "{out}");
        assert!(
            out.ends_with("               ^\njq: 1 compile error\n"),
            "{out}"
        );
    }

    #[test]
    fn a_stray_alternative_operator_is_a_syntax_error() {
        let e = program(".a?//1").expect("rejected");
        assert_eq!(e.message, "syntax error, unexpected ?//");
        assert_eq!(e.span.start, 2, "jq reports column 3");
        assert_eq!(program(". as [$a] ?// $b | $a"), None);
        assert_eq!(program(".a? // 1"), None, "with a space it is `//`");
    }

    #[test]
    fn module_metadata_must_be_a_constant_object() {
        let msg = |s: &str| module_header(s).map(|e| e.message);
        assert_eq!(
            msg("module (.+1); 0").as_deref(),
            Some("Module metadata must be constant")
        );
        assert_eq!(
            msg("module {a:.}; 0").as_deref(),
            Some("Module metadata must be constant")
        );
        assert_eq!(
            msg("module []; 0").as_deref(),
            Some("Module metadata must be an object")
        );
        assert_eq!(
            msg("module \"x\"; 0").as_deref(),
            Some("Module metadata must be an object")
        );
        assert_eq!(msg("module {a:1}; 0"), None);
        assert_eq!(msg(".a"), None);
    }
}
