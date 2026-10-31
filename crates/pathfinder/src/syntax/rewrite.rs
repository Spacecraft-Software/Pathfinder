// SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Source-to-source rewrites that make jaq run a program the way jq does.
//!
//! # Assignment
//!
//! jaq does not create missing containers when assigning through them:
//! `null | .a.b = 1` errors where jq returns `{"a":{"b":1}}`. The operators are
//! syntax, so no definition can shadow them — but jq itself compiles them into
//! calls to two builtins, and those can be supplied. Following jq 1.8.1's
//! `src/parser.y`:
//!
//! | jq writes | jq compiles to |
//! |---|---|
//! | `L = R` | `_assign(L; R)` |
//! | `L \|= R` | `_modify(L; R)` |
//! | `L op= R` | `R as $t \| _modify(L; . op $t)` |
//! | `L //= R` | `R as $t \| _modify(L; . // $t)` |
//!
//! Transcribing those definitions is correct but quadratic under jaq (jq's
//! `_modify` updates in place through a private variable jaq has no
//! equivalent of). So Pathfinder prepares the target instead — creating
//! missing containers, raising jq's error for a wrong-typed one — and then lets
//! jaq's own operator run, which agrees with jq once the containers exist. A
//! target with an explicit comma still goes through `_pf_modify`, and
//! `L |= empty` is `del(L)`, as in jq 1.7+.
//!
//! # Deletion
//!
//! `del(f)` runs on jaq's own `del` where that is jq's answer — one path per
//! array, and not a named key, which jaq removes out of order — and on jq's
//! `delpaths` algorithm otherwise. See [`Cx::delete`].
//!
//! # Syntax jaq lacks, and checks it skips
//!
//! `?//` destructuring alternatives and `{$b: pattern}` are desugared; a
//! computed object key is checked to be a string, as jq checks it. Programs jq
//! refuses to compile are found by [`super::check`], not here.
//!
//! # How text is produced
//!
//! Only rewritten nodes are reprinted. Every other byte — including the parts
//! of a rewritten node's ancestors between its children — is copied from the
//! source exactly as written. A rewritten node is always emitted inside
//! parentheses, so the binding form `R as $t | …` cannot swallow a `,` or `|`
//! that followed it.
//!
//! # Reduce and foreach sources
//!
//! jq's grammar takes a whole expression as the source of `reduce`/`foreach`
//! (`reduce .[] / .[] as $i (…)` divides first); jaq takes a postfix term and
//! rejects the form. A compound source is parenthesised.

use std::fmt::Write as _;

use super::lex::{Kw, StrPart, Tok, lex};
use super::parse::parse;
use super::{Kind, Node};

/// The result of rewriting a program body.
#[derive(Debug, Clone)]
pub struct Rewrite {
    pub text: String,
    /// Prelude definitions the rewritten text calls.
    pub needs: Vec<&'static str>,
}

/// jq 1.8.1's formats, measured: any other `@name` is jq's run-time error
/// `name is not a valid format`.
const JQ_FORMATS: [&str; 10] = [
    "text", "json", "html", "uri", "urid", "csv", "tsv", "sh", "base64", "base64d",
];

/// Environment escape hatch: set it to hand every program to jaq unparsed.
pub const NO_REWRITE_ENV: &str = "PATHFINDER_NO_REWRITE";

/// Which of jq's behaviours the rewriter supplies.
///
/// The engine has jq's semantics itself (M20); what is left is jq's deferred
/// deletion in an update of several paths, which the engine's update does
/// not do, and the syntax jaq's parser reads otherwise or not at all, for
/// the programs that parser still reads ([`Rewrites::syntax`]). `?//` is a
/// transform, not a switch, and always taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rewrites {
    /// An update of several paths (`(.a, .b) |= f`) deletes the paths `f`
    /// yields nothing for together, at the end, as jq's `_modify` does.
    pub comma_update: bool,
    /// jq's syntax jaq's parser reads otherwise or not at all: `.5`,
    /// `{$b: pattern}`, a compound `reduce`/`foreach` source, and
    /// `def f($a)` whose body calls `a`. On only when jaq's parser reads
    /// the program (an `import`/`include` header, `PATHFINDER_FRONT_END`).
    pub syntax: bool,
}

/// Rewrite `src` if it needs it; `None` means "pass it through unchanged".
///
/// Cheap when nothing applies: a token pre-screen decides whether to parse at
/// all, and most programs contain no assignment and no compound `reduce`
/// source. A program the parser rejects is also passed through — exactly what
/// happened before the parser existed.
pub fn rewrite(src: &str, on: Rewrites) -> Option<Rewrite> {
    if std::env::var_os(NO_REWRITE_ENV).is_some() || !worth_parsing(src) {
        return None;
    }
    let tree = parse(src).ok()?;
    let mut cx = Cx {
        on,
        src,
        needs: Vec::new(),
        temps: 0,
    };
    let (text, dirty) = cx.emit(&tree);
    // Keep whatever surrounds the expression — leading whitespace, a trailing
    // comment — exactly as written.
    let text = format!("{}{text}{}", &src[..tree.span.start], &src[tree.span.end..]);
    dirty.then_some(Rewrite {
        text,
        needs: cx.needs,
    })
}

/// Whether any construct this module rewrites could be present — including
/// inside string interpolations, whose tokens nest within the string token.
fn worth_parsing(src: &str) -> bool {
    fn any(src: &str, toks: &[super::lex::Token]) -> bool {
        toks.iter().any(|t| match &t.tok {
            // A number written with a bare point, `.5`, `1.` or `1.e3`,
            // which jaq cannot read.
            Tok::Number => bare_point(&src[t.span.start..t.span.end]),
            // An update (of several paths); `reduce`/`foreach` sources; `?//`
            // alternatives and `{$b: pattern}` (a binding before `:`).
            Tok::Op("|=" | "+=" | "-=" | "*=" | "/=" | "%=" | "//=" | "?//")
            | Tok::Keyword(Kw::Reduce | Kw::Foreach)
            | Tok::Binding(_) => true,
            // A format jq does not have, which it rejects at run time.
            Tok::Format(name) => !JQ_FORMATS.contains(&name.as_str()),
            Tok::Str(parts) => parts
                .iter()
                .any(|p| matches!(p, StrPart::Interp(inner, _) if any(src, inner))),
            _ => false,
        })
    }
    lex(src).is_ok_and(|toks| any(src, &toks))
}

struct Cx<'a> {
    /// The behaviours to supply.
    on: Rewrites,
    src: &'a str,
    needs: Vec<&'static str>,
    /// Counter for `$__pf_tN` temporaries, unique per program.
    temps: usize,
}

impl Cx<'_> {
    fn need(&mut self, name: &'static str) {
        if !self.needs.contains(&name) {
            self.needs.push(name);
        }
    }

    /// Emit `node`; the flag says whether anything in it was rewritten.
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per rewritten construct; the dispatch is the index of what is rewritten"
    )]
    fn emit(&mut self, node: &Node) -> (String, bool) {
        match &node.kind {
            // An update of several paths: jq's `_modify` defers deletions.
            Kind::Binary { op, lhs, rhs }
                if self.on.comma_update && is_assignment(op) && *op != "=" && has_comma(lhs) =>
            {
                let (l, _) = self.emit(lhs);
                let (r, _) = self.emit(rhs);
                (self.assignment(op, &l, &r), true)
            }
            // `.5`: jaq reads only `0.5`.
            // jaq's lexer reads neither `.5` nor `1.` (or `1.e2`); jq's
            // decNumber gives the same number without the bare point.
            Kind::Number
                if self.on.syntax && bare_point(&self.src[node.span.start..node.span.end]) =>
            {
                (
                    without_bare_point(&self.src[node.span.start..node.span.end]),
                    true,
                )
            }
            Kind::Bind {
                source,
                patterns,
                body,
            } if patterns.len() > 1
                || (self.on.syntax && patterns.iter().any(binds_and_destructures)) =>
            {
                (self.bind(source, patterns, body), true)
            }
            Kind::Reduce {
                source,
                patterns,
                init,
                update,
            } if self.on.syntax && patterns.len() == 1 && binds_and_destructures(&patterns[0]) => {
                let (s, _) = self.emit(source);
                let (i, _) = self.emit(init);
                let (u, _) = self.emit(update);
                let p = pattern_text(&patterns[0], self.src);
                (format!("reduce ({s}) as {p} ({i}; {u})"), true)
            }
            Kind::Foreach {
                source,
                patterns,
                init,
                update,
                extract,
            } if self.on.syntax && patterns.len() == 1 && binds_and_destructures(&patterns[0]) => {
                let (src, _) = self.emit(source);
                let (init, _) = self.emit(init);
                let (update, _) = self.emit(update);
                let extract = extract
                    .as_deref()
                    .map_or_else(String::new, |x| format!("; {}", self.emit(x).0));
                let pat = pattern_text(&patterns[0], self.src);
                (
                    format!("foreach ({src}) as {pat} ({init}; {update}{extract})"),
                    true,
                )
            }
            // `def f($a): body` binds `a` as a filter too in jq, which compiles
            // it as `def f(a): a as $a | body`; jaq binds only `$a`. Written
            // out only when the body calls a value parameter as a filter.
            Kind::Def { def, rest }
                if self.on.syntax
                    && def.params.iter().any(|p| match p {
                        super::Param::Value(v) => calls(&def.body, v.trim_start_matches('$')),
                        super::Param::Filter(_) => false,
                    }) =>
            {
                let names: Vec<&str> = def
                    .params
                    .iter()
                    .map(|p| match p {
                        super::Param::Filter(n) | super::Param::Value(n) => {
                            n.trim_start_matches('$')
                        }
                    })
                    .collect();
                let binds: String = def
                    .params
                    .iter()
                    .filter_map(|p| match p {
                        super::Param::Value(v) => {
                            let v = v.trim_start_matches('$');
                            Some(format!("{v} as ${v} | "))
                        }
                        super::Param::Filter(_) => None,
                    })
                    .collect();
                let (body, _) = self.emit(&def.body);
                let (rest, _) = self.emit(rest);
                (
                    format!(
                        "(def {}({}): {binds}({body}); {rest})",
                        def.name,
                        names.join("; ")
                    ),
                    true,
                )
            }
            Kind::Reduce { source, .. } | Kind::Foreach { source, .. }
                if self.on.syntax && is_compound(source) =>
            {
                (self.splice_wrapping(node, Some(source.span)).0, true)
            }
            // jq answers an unknown format at run time; the engine has none
            // (or formats of its own, such as jaq's `@htmld`).
            Kind::Format(f) if !JQ_FORMATS.contains(&f.as_str()) => {
                (format!("(error(\"{f} is not a valid format\"))"), true)
            }
            // A format applies only to interpolations: without one, the
            // string is the string.
            Kind::Str(s)
                if s.format
                    .as_deref()
                    .is_some_and(|f| !JQ_FORMATS.contains(&f)) =>
            {
                let f = s.format.as_deref().unwrap_or_default();
                if s.parts
                    .iter()
                    .any(|p| matches!(p, super::StrSeg::Interp(_)))
                {
                    (format!("(error(\"{f} is not a valid format\"))"), true)
                } else {
                    let text = &self.src[node.span.start..node.span.end];
                    (text[text.find('"').unwrap_or(0)..].to_owned(), true)
                }
            }
            _ => self.splice(node),
        }
    }

    /// Copy `node`'s source, substituting the emitted text of each rewritten
    /// child. Untouched nodes come back byte-identical.
    fn splice(&mut self, node: &Node) -> (String, bool) {
        self.splice_wrapping(node, None)
    }

    /// [`Self::splice`], additionally parenthesising the child at `wrap`.
    fn splice_wrapping(&mut self, node: &Node, wrap: Option<super::Span>) -> (String, bool) {
        self.splice_mapping(node, wrap, |t| format!("({t})"))
    }

    /// [`Self::splice`], replacing the emitted text of the child at `target`
    /// with `f` applied to it.
    fn splice_mapping(
        &mut self,
        node: &Node,
        target: Option<super::Span>,
        f: impl Fn(&str) -> String,
    ) -> (String, bool) {
        let span = node.span;
        let mut out = String::with_capacity(span.end - span.start);
        let mut cursor = span.start;
        let mut dirty = false;
        // `.a.[0]` and `.a.[]` are `.a[0]` and `.a[]`, which jaq 3.0 does not
        // parse; the `.` after the base is dropped. Any later gap is copied.
        let undot = matches!(node.kind, Kind::Index { .. } | Kind::Iterate(_));
        let gap = |out: &mut String, from: usize, to: usize, first: bool| {
            let text = &self.src[from..to];
            match text.trim_start().strip_prefix('.') {
                Some(rest) if first && undot && rest.trim_start().starts_with('[') => {
                    out.push_str(rest);
                    true
                }
                _ => {
                    out.push_str(text);
                    false
                }
            }
        };
        let mut first = false;
        for child in node.children() {
            // Zero-width synthetic children (the `.` implied by `.foo`) have no
            // text of their own and can never be rewritten.
            if child.span.start == child.span.end {
                continue;
            }
            let (text, child_dirty) = self.emit(child);
            dirty |= gap(&mut out, cursor, child.span.start, first);
            first = cursor == span.start && child.span.start == span.start;
            if target == Some(child.span) {
                out.push_str(&f(&text));
                dirty = true;
            } else {
                out.push_str(&text);
            }
            cursor = child.span.end;
            dirty |= child_dirty;
        }
        dirty |= gap(&mut out, cursor, span.end, first);
        if dirty {
            (out, true)
        } else {
            (self.src[span.start..span.end].to_owned(), false)
        }
    }

    /// `source as p1 ?// p2 … | body`, which jaq does not parse.
    ///
    /// jq tries each pattern in turn; an error while destructuring *or in the
    /// body* moves to the next alternative, outputs already produced are kept,
    /// and only the last alternative's error escapes. Every variable named in
    /// any alternative is bound in all of them, `null` where its own pattern
    /// does not bind it. That is `try … catch` over the alternatives, with the
    /// source evaluated once and the variables pre-bound to `null`.
    fn bind(&mut self, source: &Node, patterns: &[super::Pattern], body: &Node) -> String {
        let (s, _) = self.emit(source);
        let (b, _) = self.emit(body);
        let pats: Vec<String> = patterns.iter().map(|p| pattern_text(p, self.src)).collect();
        if let [only] = pats.as_slice() {
            return format!("({s} as {only} | {b})");
        }
        let mut vars: Vec<&str> = Vec::new();
        for p in patterns {
            pattern_vars(p, &mut vars);
        }
        let nulls = vars.iter().fold(String::new(), |mut out, v| {
            let _ = write!(out, "null as ${v} | ");
            out
        });
        let v = format!("$__pf_t{}", self.temps);
        // The body is written once, as a definition taking every variable:
        // pasted into each alternative, a chain of `?//` grew threefold per
        // link (found by fuzzing, M24).
        let f = format!("_pf_alt{}", self.temps);
        self.temps += 1;
        // `($a; $b)`: the parameters, and the arguments passing them on.
        let args = if vars.is_empty() {
            String::new()
        } else {
            let list: Vec<String> = vars.iter().map(|v| format!("${v}")).collect();
            format!("({})", list.join("; "))
        };
        // jaq reads `.[0]` of an object as `null` where jq raises an error, so
        // an array pattern would match an object and the next alternative would
        // never be tried. Check first, with jq's message.
        let alt = |p: &str, pat: &super::Pattern| {
            let check =
                array_check(pat).map_or_else(String::new, |c| format!("({v} | {c} | empty), "));
            format!("({nulls}{check}({v} as {p} | {f}{args}))")
        };
        let mut alts = pats.iter().zip(patterns).rev();
        let (last, last_pat) = alts.next().expect("more than one pattern");
        let mut text = alt(last, last_pat);
        for (p, pat) in alts {
            text = format!("(try {} catch {text})", alt(p, pat));
        }
        format!("({s} as {v} | def {f}{args}: {b}; {text})")
    }

    /// Emit an update of several paths through jq's own `_modify`: the
    /// engine deletes the paths `f` yields nothing for one by one, shifting
    /// later indices (`.[1,2] |= empty`), where jq deletes them together at
    /// the end. `_modify` is slow only with many paths, and an explicit comma
    /// never makes many. `op=` evaluates its right-hand side once, on the
    /// original input, as jq does.
    fn assignment(&mut self, op: &str, l: &str, r: &str) -> String {
        let t = format!("$__pf_t{}", self.temps);
        self.temps += 1;
        self.need("_pf_modify");
        if op == "|=" {
            format!("_pf_modify(({l}); ({r}))")
        } else {
            let bin = &op[..op.len() - 1];
            format!("(({r}) as {t} | _pf_modify(({l}); . {bin} {t}))")
        }
    }
}

/// A filter that raises jq's error where `pat` would index an object by
/// position, or `None` when the pattern has no array part. It yields `.`
/// otherwise; the caller discards it.
fn array_check(pat: &super::Pattern) -> Option<String> {
    use super::{ObjKey, ObjPat, Pattern};
    match pat {
        Pattern::Var(_) => None,
        Pattern::Array(items) => {
            let inner: Vec<String> = items
                .iter()
                .enumerate()
                .filter_map(|(i, it)| array_check(it).map(|c| format!("(.[{i}] | {c})")))
                .collect();
            let then = if inner.is_empty() {
                ".".to_owned()
            } else {
                inner.join(", ")
            };
            Some(format!(
                "(if type == \"object\" then error(\"Cannot index object with number\") else {then} end)"
            ))
        }
        Pattern::Object(items) => {
            let inner: Vec<String> = items
                .iter()
                .filter_map(|it| {
                    let (key, p) = match it {
                        ObjPat::VarPattern(k, p) | ObjPat::Key(ObjKey::Ident(k), p) => {
                            (format!("\"{k}\""), p)
                        }
                        // A computed or variable key is evaluated against the
                        // outer input; leave that subtree unchecked.
                        ObjPat::Var(_) | ObjPat::Key(_, _) => return None,
                    };
                    array_check(p).map(|c| format!("(.[{key}] | {c})"))
                })
                .collect();
            (!inner.is_empty()).then(|| inner.join(", "))
        }
    }
}

/// A pattern spelled so that jaq parses it.
fn pattern_text(p: &super::Pattern, src: &str) -> String {
    let mut o = String::new();
    super::print::pattern(p, src, &mut o);
    o
}

/// Whether a pattern uses `{$b: pattern}`, which jaq does not parse.
fn binds_and_destructures(p: &super::Pattern) -> bool {
    use super::{ObjPat, Pattern};
    match p {
        Pattern::Var(_) => false,
        Pattern::Array(items) => items.iter().any(binds_and_destructures),
        Pattern::Object(items) => items.iter().any(|it| match it {
            ObjPat::Var(_) => false,
            ObjPat::VarPattern(..) => true,
            ObjPat::Key(_, p) => binds_and_destructures(p),
        }),
    }
}

/// The variables a pattern binds, in order, without repeats.
fn pattern_vars<'a>(p: &'a super::Pattern, out: &mut Vec<&'a str>) {
    use super::{ObjPat, Pattern};
    let add = |v: &'a str, out: &mut Vec<&'a str>| {
        if !out.contains(&v) {
            out.push(v);
        }
    };
    match p {
        Pattern::Var(v) => add(v, out),
        Pattern::Array(items) => items.iter().for_each(|it| pattern_vars(it, out)),
        Pattern::Object(items) => {
            for it in items {
                match it {
                    ObjPat::Var(v) => add(v, out),
                    ObjPat::VarPattern(v, p) => {
                        add(v, out);
                        pattern_vars(p, out);
                    }
                    ObjPat::Key(_, p) => pattern_vars(p, out),
                }
            }
        }
    }
}

/// Whether an assignment target spells out several paths with a comma.
fn has_comma(node: &Node) -> bool {
    matches!(node.kind, Kind::Comma(..)) || node.children().into_iter().any(has_comma)
}

/// Whether `node` calls `name` with no arguments anywhere.
fn calls(node: &Node, name: &str) -> bool {
    matches!(&node.kind, Kind::Call { name: n, args } if n == name && args.is_empty())
        || node.children().into_iter().any(|c| calls(c, name))
}

fn is_assignment(op: &str) -> bool {
    matches!(op, "=" | "|=" | "+=" | "-=" | "*=" | "/=" | "%=" | "//=")
}

/// A `reduce`/`foreach` source jaq would not parse as written: anything that
/// is not a single postfix term.
const fn is_compound(source: &Node) -> bool {
    matches!(source.kind, Kind::Binary { .. } | Kind::Neg(_))
}

/// Whether a number literal has a point with no digit before or after it.
fn bare_point(n: &str) -> bool {
    let b = n.as_bytes();
    b.iter().enumerate().any(|(i, &c)| {
        c == b'.'
            && (i == 0
                || !b[i - 1].is_ascii_digit()
                || !b.get(i + 1).is_some_and(u8::is_ascii_digit))
    })
}

/// The literal as jaq reads it: `.5` is `0.5`, `1.` is `1`, `1.e2` is `1e2`.
fn without_bare_point(n: &str) -> String {
    let mut out = String::with_capacity(n.len() + 1);
    let b = n.as_bytes();
    for (i, &c) in b.iter().enumerate() {
        if c == b'.' {
            let before = i > 0 && b[i - 1].is_ascii_digit();
            let after = b.get(i + 1).is_some_and(u8::is_ascii_digit);
            if !before {
                out.push('0');
            }
            if !after {
                continue;
            }
        }
        out.push(char::from(c));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{Rewrites, rewrite};

    /// The rewrites a program takes when jaq's parser reads it.
    fn rw(src: &str) -> Option<String> {
        let on = Rewrites {
            comma_update: true,
            syntax: true,
        };
        rewrite(src, on).map(|r| r.text)
    }

    /// The rewrites a program takes when Pathfinder's parser reads it.
    fn rw_native(src: &str) -> Option<String> {
        let on = Rewrites {
            comma_update: true,
            syntax: false,
        };
        rewrite(src, on).map(|r| r.text)
    }

    #[test]
    fn programs_without_a_comma_update_are_not_touched() {
        assert_eq!(rw(".a | select(.b == 1)"), None);
        assert_eq!(rw("[.[] | . + 1]"), None);
        assert_eq!(rw(".a == 1 and .b <= 2 or .c != 3"), None);
        // The engine's own update is jq's for a single path.
        assert_eq!(rw(".a.b = 1"), None);
        assert_eq!(rw(".a |= . + 1"), None);
        assert_eq!(rw("del(.[0,1])"), None);
        assert_eq!(rw("(.a, .b) = 1"), None, "`=` defers nothing");
    }

    #[test]
    fn a_comma_target_goes_through_jqs_modify() {
        // The engine's update deletes `.[1,2]` one after the other, shifting
        // the second.
        assert_eq!(
            rw(".[1,2] += 1").as_deref(),
            Some("((1) as $__pf_t0 | _pf_modify((.[1,2]); . + $__pf_t0))")
        );
        assert_eq!(
            rw("(.a, .b) |= empty").as_deref(),
            Some("_pf_modify(((.a, .b)); (empty))")
        );
    }

    #[test]
    fn surrounding_text_is_kept_byte_for_byte() {
        // Odd spacing and the comment survive; only the update changed.
        let out = rw("[ .x ,  ((.a,.b) |= 1) ]  # keep me").expect("rewritten");
        assert!(out.starts_with("[ .x ,  (_pf_modify("), "{out}");
        assert!(out.ends_with(") ]  # keep me"), "{out}");
    }

    #[test]
    fn a_rewritten_binding_cannot_swallow_what_follows() {
        // Without the outer parentheses, `R as $t | …` would take `, 2` along.
        let out = rw("[.[0,1] += 1, 2]").expect("rewritten");
        assert!(out.starts_with("[((1) as $__pf_t0 |"), "{out}");
        assert!(out.ends_with("), 2]"), "{out}");
    }

    #[test]
    fn nested_updates_get_distinct_temporaries() {
        let out = rw(".[0,1] += (.[2,3] += 1 | .[0])").expect("rewritten");
        assert!(
            out.contains("$__pf_t0") && out.contains("$__pf_t1"),
            "{out}"
        );
    }

    #[test]
    fn updates_inside_strings_and_defs_are_found() {
        assert!(rw("\"\\((.a,.b) |= 1)\"").is_some());
        assert!(rw("def f: (.a,.b) |= 1; f").is_some());
        assert!(rw("{k: ((.a,.b) |= 2)}").is_some());
    }

    #[test]
    fn a_value_parameter_called_as_a_filter_is_desugared_for_jaqs_parser() {
        assert_eq!(
            rw("def f($a; g): [a, $a, g]; f(1)").as_deref(),
            Some("(def f(a; g): a as $a | ([a, $a, g]); f(1))")
        );
        // Not called as a filter: left as written.
        assert_eq!(rw("def f($a): $a + 1; f(1)"), None);
        // Pathfinder's parser reads jq's syntax itself.
        assert_eq!(rw_native("def f($a; g): [a, $a, g]; f(1)"), None);
    }

    #[test]
    fn compound_reduce_sources_are_parenthesised_for_jaqs_parser() {
        assert_eq!(
            rw("reduce .[] * 2 as $i (0; . + $i)").as_deref(),
            Some("reduce (.[] * 2) as $i (0; . + $i)")
        );
        assert_eq!(
            rw("reduce .[] as $i (0; . + $i)"),
            None,
            "a simple source is left alone"
        );
        assert_eq!(rw_native("reduce .[] * 2 as $i (0; . + $i)"), None);
    }

    #[test]
    fn bare_points_are_written_out_for_jaqs_parser() {
        assert_eq!(rw("[.5, 1., 1.e3]").as_deref(), Some("[0.5, 1, 1e3]"));
        assert_eq!(rw_native("[.5, 1., 1.e3]"), None);
    }

    #[test]
    fn an_unknown_format_is_jqs_run_time_error() {
        assert_eq!(
            rw_native("@nope").as_deref(),
            Some("(error(\"nope is not a valid format\"))")
        );
        assert_eq!(rw_native("@base64d"), None);
    }

    #[test]
    fn unparseable_programs_pass_through() {
        assert_eq!(rw(".[0,1] |= "), None);
    }

    #[test]
    fn a_destructuring_body_is_written_once() {
        let once = rw_native(".[] as [$a] ?// $a | $a + 1000").expect("rewritten");
        assert_eq!(once.matches("1000").count(), 1, "{once}");
        // Pasted into each alternative, 20 links were 3^20 copies.
        let link = ".[] as $a ?// {a:$a} ?// {a:$a}";
        let src = format!("{} | $a", vec![link; 20].join(" | "));
        let text = rw_native(&src).expect("rewritten");
        assert!(text.len() < 40 * src.len(), "{} bytes", text.len());
    }
}
