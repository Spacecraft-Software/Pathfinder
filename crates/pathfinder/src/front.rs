// SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The native engine's front end: the program as Pathfinder's parser reads
//! it, handed to the engine's compiler as its own terms (PLAN M21, P-068).
//!
//! The engine's loader takes [`main_module`] in place of jaq's parser
//! (`jaq::Cli::front_end`). Pathfinder's tree is jq's grammar, so the
//! engine compiles the program jq would compile; where the parser passes a
//! program through (a construct it does not know, or a module header),
//! [`main_module`] answers `None` and jaq's parser reads it, as before M21.
//!
//! Every string in the terms is a slice of the program text: the engine
//! finds a name's position in the program from where its slice points. A
//! use (a call, a variable, a literal) is the slice its node spans; a name
//! that only binds (a parameter, a pattern variable) is compared by its
//! text alone, so any slice that spells it serves.

use jaq::jaq_core::load::lex::StrPart;
use jaq::jaq_core::load::parse::{BinaryOp, Def, Module, Pattern as JPattern, Term};
use jaq::jaq_core::ops::{Cmp, Math};
use jaq::jaq_core::path::{Opt, Part, Path};

use crate::syntax::{
    FieldName, FuncDef, Kind, Node, ObjKey, ObjPair, ObjPat, ObjShort, Param, Pattern, Span,
    StrLit, StrSeg, parse::parse,
};

/// Programs longer than this are converted on a thread of [`DEEP_STACK`],
/// as `program::assemble` does: parsing and converting recurse.
const DEEP_SOURCE: usize = 4096;

/// The stack for a long program: address space reserved, not memory used.
const DEEP_STACK: usize = 256 << 20;

/// Set to `jaq` to have jaq's parser read every program, to compare the two.
pub const FRONT_END_ENV: &str = "PATHFINDER_FRONT_END";

/// The front end the native engine runs: Pathfinder's, unless
/// [`FRONT_END_ENV`] asks for jaq's.
pub fn selected() -> Option<jaq::jaq_core::load::MainParser> {
    let jaqs = std::env::var_os(FRONT_END_ENV).is_some_and(|v| v == "jaq");
    (!jaqs).then_some(main_module as jaq::jaq_core::load::MainParser)
}

/// The program as the engine's main module, or `None` for jaq's parser.
pub fn main_module(src: &str) -> Option<Module<&str, Term<&str>>> {
    if src.len() <= DEEP_SOURCE {
        return convert(src);
    }
    std::thread::scope(|scope| {
        let deep = std::thread::Builder::new()
            .stack_size(DEEP_STACK)
            .spawn_scoped(scope, || convert(src));
        match deep {
            Ok(handle) => handle
                .join()
                .unwrap_or_else(|panic| std::panic::resume_unwind(panic)),
            // No thread to be had: jaq's parser reads it, as before M21.
            Err(_) => None,
        }
    })
}

fn convert(src: &str) -> Option<Module<&str, Term<&str>>> {
    let tree = parse(src).ok()?;
    let body = Cx { src }.term(&tree)?;
    Some(Module {
        meta: None,
        deps: Vec::new(),
        body,
    })
}

struct Cx<'s> {
    src: &'s str,
}

impl<'s> Cx<'s> {
    /// The text a span covers.
    fn at(&self, span: Span) -> Option<&'s str> {
        self.src.get(span.start..span.end)
    }

    /// The keyword a node starts with.
    fn keyword(&self, span: Span, keyword: &str) -> Option<&'s str> {
        self.src
            .get(span.start..span.start + keyword.len())
            .filter(|at| *at == keyword)
    }

    /// A slice spelling `text`: within `near` if it is there, else anywhere.
    fn spelled(&self, text: &str, near: Span) -> Option<&'s str> {
        let find = |from: usize, to: usize| {
            let hay = self.src.get(from..to)?;
            hay.find(text).map(|i| from + i)
        };
        let at = find(near.start, near.end).or_else(|| find(0, self.src.len()))?;
        self.src.get(at..at + text.len())
    }

    /// A `$` name: `name` is written without its `$`.
    fn binder(&self, name: &str, near: Span) -> Option<&'s str> {
        self.spelled(&format!("${name}"), near)
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one arm per grammar construct; the match is the index of the conversion"
    )]
    fn term(&self, node: &Node) -> Option<Term<&'s str>> {
        let bin = |l: &Node, op, r: &Node| -> Option<Term<&'s str>> {
            Some(Term::BinOp(
                Box::new(self.term(l)?),
                op,
                Box::new(self.term(r)?),
            ))
        };
        Some(match &node.kind {
            Kind::Pipe(l, r) => bin(l, BinaryOp::Pipe(None), r)?,
            Kind::Comma(l, r) => bin(l, BinaryOp::Comma, r)?,
            Kind::Bind {
                source,
                patterns,
                body,
            } => {
                // `?//` alternatives are rewritten before the engine sees them.
                let [pattern] = &**patterns else {
                    return None;
                };
                let pattern = self.pattern(pattern, node.span)?;
                bin(source, BinaryOp::Pipe(Some(pattern)), body)?
            }
            Kind::Label { name, body } => {
                Term::Label(self.binder(name, node.span)?, Box::new(self.term(body)?))
            }
            Kind::Def { def, rest } => Term::Def(
                Vec::from([self.def(def, node.span)?]),
                Box::new(self.term(rest)?),
            ),
            Kind::Binary { op, lhs, rhs } => bin(lhs, binary_op(op)?, rhs)?,
            Kind::Neg(n) => Term::Neg(Box::new(self.term(n)?)),
            Kind::Identity => Term::Id,
            Kind::Recurse => Term::Recurse,
            Kind::Number => Term::Num(self.at(node.span)?),
            Kind::Str(s) => self.string(s, node.span)?,
            Kind::Format(_) => Term::Call(self.at(node.span)?, Vec::new()),
            Kind::Var(_) => Term::Var(self.at(node.span)?),
            // `$__loc__` is replaced before the engine sees it.
            Kind::Loc => return None,
            Kind::Break(name) => Term::Break(self.binder(name, node.span)?),
            Kind::Call { name, args } => {
                let name = self
                    .src
                    .get(node.span.start..node.span.start + name.len())
                    .filter(|at| at == name)?;
                let args = args.iter().map(|a| self.term(a)).collect::<Option<_>>()?;
                Term::Call(name, args)
            }
            Kind::Paren(n) => self.term(n)?,
            Kind::Array(None) => Term::Arr(None),
            Kind::Array(Some(n)) => Term::Arr(Some(Box::new(self.term(n)?))),
            Kind::Object(pairs) => Term::Obj(
                pairs
                    .iter()
                    .map(|p| self.obj_pair(p, node.span))
                    .collect::<Option<_>>()?,
            ),
            Kind::Reduce {
                source,
                patterns,
                init,
                update,
            } => self.fold(node, "reduce", source, patterns, &[init, update])?,
            Kind::Foreach {
                source,
                patterns,
                init,
                update,
                extract,
            } => {
                let mut args = Vec::from([&**init, &**update]);
                args.extend(extract.as_deref());
                self.fold(node, "foreach", source, patterns, &args)?
            }
            Kind::If {
                branches,
                otherwise,
            } => Term::IfThenElse(
                branches
                    .iter()
                    .map(|(c, t)| Some((self.term(c)?, self.term(t)?)))
                    .collect::<Option<_>>()?,
                match otherwise {
                    Some(e) => Some(Box::new(self.term(e)?)),
                    None => None,
                },
            ),
            Kind::Try { body, handler } => Term::TryCatch(
                Box::new(self.term(body)?),
                match handler {
                    Some(h) => Some(Box::new(self.term(h)?)),
                    None => None,
                },
            ),
            Kind::Field { .. } | Kind::Index { .. } | Kind::Slice { .. } | Kind::Iterate(_) => {
                self.step(node, Opt::Essential)?
            }
            // jq's `?` after a postfix step makes that step optional
            // (`1 | .a.b?` still fails at `.a`); after anything else,
            // `?` included, it is `try`.
            Kind::Optional(n) => match &n.kind {
                Kind::Field { .. } | Kind::Index { .. } | Kind::Slice { .. } | Kind::Iterate(_) => {
                    self.step(n, Opt::Optional)?
                }
                _ => Term::TryCatch(Box::new(self.term(n)?), None),
            },
        })
    }

    /// `reduce` or `foreach`: the keyword, the source, its one pattern (`?//`
    /// alternatives are rewritten before the engine sees them), the rest.
    fn fold(
        &self,
        node: &Node,
        keyword: &str,
        source: &Node,
        patterns: &[Pattern],
        args: &[&Node],
    ) -> Option<Term<&'s str>> {
        let [pattern] = patterns else {
            return None;
        };
        Some(Term::Fold(
            self.keyword(node.span, keyword)?,
            Box::new(self.term(source)?),
            self.pattern(pattern, node.span)?,
            args.iter().map(|a| self.term(a)).collect::<Option<_>>()?,
        ))
    }

    /// A postfix step on its base, joined to the base's own path.
    fn step(&self, node: &Node, opt: Opt) -> Option<Term<&'s str>> {
        let (base, part) = match &node.kind {
            Kind::Field { base, name } => {
                let key = match name {
                    FieldName::Ident(name) => {
                        let at = node.span.end.checked_sub(name.len())?;
                        literal(self.src.get(at..node.span.end)?)
                    }
                    FieldName::Str(s) => self.string(s, node.span)?,
                };
                (base, Part::Index(key))
            }
            Kind::Index { base, index } => (base, Part::Index(self.term(index)?)),
            Kind::Slice { base, from, to } => {
                let bound = |b: &Option<Box<Node>>| match b {
                    Some(b) => self.term(b).map(Some),
                    None => Some(None),
                };
                (base, Part::Range(bound(from)?, bound(to)?))
            }
            Kind::Iterate(base) => (base, Part::Range(None, None)),
            _ => return None,
        };
        Some(match self.term(base)? {
            Term::Path(base, Path(mut parts)) => {
                parts.push((part, opt));
                Term::Path(base, Path(parts))
            }
            base => Term::Path(Box::new(base), Path(Vec::from([(part, opt)]))),
        })
    }

    fn def(&self, def: &FuncDef, near: Span) -> Option<Def<&'s str>> {
        let name = self.spelled(&def.name, near)?;
        let args = def
            .params
            .iter()
            .map(|p| match p {
                Param::Filter(f) => self.spelled(f, near),
                Param::Value(v) => self.binder(v, near),
            })
            .collect::<Option<_>>()?;
        Some(Def {
            name,
            args,
            body: self.term(&def.body)?,
        })
    }

    fn pattern(&self, p: &Pattern, near: Span) -> Option<JPattern<&'s str>> {
        Some(match p {
            Pattern::Var(v) => JPattern::Var(self.binder(v, near)?),
            Pattern::Array(ps) => JPattern::Arr(
                ps.iter()
                    .map(|p| self.pattern(p, near))
                    .collect::<Option<_>>()?,
            ),
            Pattern::Object(ps) => JPattern::Obj(
                ps.iter()
                    .map(|p| self.obj_pattern(p, near))
                    .collect::<Option<_>>()?,
            ),
        })
    }

    fn obj_pattern(&self, p: &ObjPat, near: Span) -> Option<(Term<&'s str>, JPattern<&'s str>)> {
        Some(match p {
            // `{$a}` binds `$a` to `.a`.
            ObjPat::Var(v) => {
                let var = self.binder(v, near)?;
                (literal(&var[1..]), JPattern::Var(var))
            }
            // `{$a: p}` binds `$a` to `.a` and destructures it with `p`;
            // jaq has no such entry, so the object is matched twice.
            ObjPat::VarPattern(..) => return None,
            ObjPat::Key(k, p) => (self.obj_key(k, near)?, self.pattern(p, near)?),
        })
    }

    fn obj_key(&self, k: &ObjKey, near: Span) -> Option<Term<&'s str>> {
        Some(match k {
            ObjKey::Ident(name) => literal(self.spelled(name, near)?),
            ObjKey::Str(s) => self.string(s, near)?,
            ObjKey::Var(v) => Term::Var(self.binder(v, near)?),
            ObjKey::Expr(e) => self.term(e)?,
        })
    }

    fn obj_pair(&self, p: &ObjPair, near: Span) -> Option<(Term<&'s str>, Option<Term<&'s str>>)> {
        Some(match p {
            ObjPair::KeyValue { key, value } => (self.obj_key(key, near)?, Some(self.term(value)?)),
            ObjPair::Shorthand(ObjShort::Ident(name)) => (literal(self.spelled(name, near)?), None),
            ObjPair::Shorthand(ObjShort::Str(s)) => (self.string(s, near)?, None),
            ObjPair::Shorthand(ObjShort::Var(v)) => (Term::Var(self.binder(v, near)?), None),
            ObjPair::Shorthand(ObjShort::Loc(_)) => return None,
        })
    }

    /// A string literal: its text in runs without escapes, each escape as
    /// the character it stands for, read as jq's lexer reads them.
    fn string(&self, s: &StrLit, near: Span) -> Option<Term<&'s str>> {
        let format = match &s.format {
            Some(f) => Some(self.spelled(&format!("@{f}"), near)?),
            None => None,
        };
        let mut parts = Vec::new();
        for seg in &s.parts {
            match seg {
                StrSeg::Text(span) => self.unescape(*span, &mut parts)?,
                StrSeg::Interp(n) => parts.push(StrPart::Term(self.term(n)?)),
            }
        }
        Some(Term::Str(format, parts))
    }

    fn unescape(&self, span: Span, out: &mut Vec<StrPart<&'s str, Term<&'s str>>>) -> Option<()> {
        let text = self.at(span)?;
        let b = text.as_bytes();
        let (mut run, mut i) = (0, 0);
        while i < b.len() {
            if b[i] != b'\\' {
                i += 1;
                continue;
            }
            if run < i {
                out.push(StrPart::Str(&text[run..i]));
            }
            let (c, len) = match b.get(i + 1)? {
                b'"' => ('"', 2),
                b'\\' => ('\\', 2),
                b'/' => ('/', 2),
                b'b' => ('\u{8}', 2),
                b'f' => ('\u{c}', 2),
                b'n' => ('\n', 2),
                b'r' => ('\r', 2),
                b't' => ('\t', 2),
                b'u' => unicode(text.get(i + 2..)?)?,
                _ => return None,
            };
            out.push(StrPart::Char(c));
            i += len;
            run = i;
        }
        if run < b.len() {
            out.push(StrPart::Str(&text[run..]));
        }
        Some(())
    }
}

/// A `\u` escape after its `\u`, and the length of the whole escape: a
/// surrogate pair is one character, a lone low surrogate U+FFFD (jq's
/// reading), and a lone high surrogate is jq's compile error, left to the
/// engine's parser.
fn unicode(after: &str) -> Option<(char, usize)> {
    let hex = |s: &str| {
        let digits = s.get(..4)?;
        if digits.bytes().all(|b| b.is_ascii_hexdigit()) {
            u32::from_str_radix(digits, 16).ok()
        } else {
            None
        }
    };
    let first = hex(after)?;
    match first {
        0xD800..=0xDBFF => {
            let low = after.get(4..)?.strip_prefix("\\u").and_then(hex)?;
            if !(0xDC00..=0xDFFF).contains(&low) {
                return None;
            }
            let c = 0x10000 + ((first - 0xD800) << 10) + (low - 0xDC00);
            Some((char::from_u32(c)?, 12))
        }
        0xDC00..=0xDFFF => Some(('\u{FFFD}', 6)),
        c => Some((char::from_u32(c)?, 6)),
    }
}

/// A string literal without escapes: an object key, a field name.
fn literal<S>(text: S) -> Term<S> {
    Term::Str(None, Vec::from([StrPart::Str(text)]))
}

/// The engine's operator for a binary operator's spelling.
fn binary_op<S>(op: &str) -> Option<BinaryOp<S>> {
    Some(match op {
        "+" => BinaryOp::Math(Math::Add),
        "-" => BinaryOp::Math(Math::Sub),
        "*" => BinaryOp::Math(Math::Mul),
        "/" => BinaryOp::Math(Math::Div),
        "%" => BinaryOp::Math(Math::Rem),
        "=" => BinaryOp::Assign,
        "|=" => BinaryOp::Update,
        "+=" => BinaryOp::UpdateMath(Math::Add),
        "-=" => BinaryOp::UpdateMath(Math::Sub),
        "*=" => BinaryOp::UpdateMath(Math::Mul),
        "/=" => BinaryOp::UpdateMath(Math::Div),
        "%=" => BinaryOp::UpdateMath(Math::Rem),
        "//=" => BinaryOp::UpdateAlt,
        "<" => BinaryOp::Cmp(Cmp::Lt),
        ">" => BinaryOp::Cmp(Cmp::Gt),
        "<=" => BinaryOp::Cmp(Cmp::Le),
        ">=" => BinaryOp::Cmp(Cmp::Ge),
        "==" => BinaryOp::Cmp(Cmp::Eq),
        "!=" => BinaryOp::Cmp(Cmp::Ne),
        "//" => BinaryOp::Alt,
        "or" => BinaryOp::Or,
        "and" => BinaryOp::And,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::{Term, convert};
    use jaq::jaq_core::load::lex::StrPart;
    use jaq::jaq_core::load::parse::Pattern;
    use jaq::jaq_core::path::Part;

    #[test]
    fn every_construct_converts() {
        for src in [
            ".",
            "..",
            "1, 2 | 3",
            ".a.b?[0][1:2][]?.\"c\\n\"",
            "$x[0] | .[.a]",
            "-1 + 2 * 3 / 4 % 5 - 6",
            ". as [$a, {b: $c, $d, (\"e\"): [$f]}] | $a",
            "label $out | 1, break $out",
            "def f(g; $h): g + $h; f(.; 1)",
            "reduce .[] as $x (0; . + $x)",
            "foreach .[] as $x (0; . + $x; [$x, .])",
            "if . then 1 elif 2 then 3 else 4 end",
            "if . then 1 end",
            "try error(1) catch .",
            "(1, 2)?",
            ".a.b??",
            "[] | {} | [1] | {a: 1, \"b\": 2, (\"c\"): 3, $x, d, \"e\", @base64 \"k\": 4}",
            "@base64 \"x\\(.)y\" | @json",
            "\"\\u00e9\\ud83d\\ude00\\ude00\\t\"",
            ".a = 1 | .b |= 2 | .c += 3 | .d //= 4 | .e // 5",
            ". == 1 and . != 2 or . < 3",
            "a::b(1)",
        ] {
            assert!(convert(src).is_some(), "{src}");
        }
    }

    #[test]
    fn headers_and_what_jq_rejects_are_left_to_jaqs_parser() {
        for src in [
            "import \"a\" as a; .",
            "include \"a\"; .",
            "module {}; .",
            "\"\\v\"",
            "\"\\ud83d\"",
            "1 +",
            "{$__loc__}",
        ] {
            assert!(convert(src).is_none(), "{src}");
        }
    }

    /// Every string in a term, so that a test can check where it points.
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per term; splitting them apart hides that none is missed"
    )]
    fn strings<'a>(t: &Term<&'a str>, out: &mut Vec<&'a str>) {
        let pattern = |p: &Pattern<&'a str>, out: &mut Vec<&'a str>| {
            fn walk<'a>(p: &Pattern<&'a str>, out: &mut Vec<&'a str>) {
                match p {
                    Pattern::Var(v) => out.push(v),
                    Pattern::Arr(ps) => {
                        for p in ps {
                            walk(p, out);
                        }
                    }
                    Pattern::Obj(ps) => {
                        for (k, p) in ps {
                            strings(k, out);
                            walk(p, out);
                        }
                    }
                }
            }
            walk(p, out);
        };
        match t {
            Term::Num(s) | Term::Var(s) | Term::Break(s) => out.push(s),
            Term::Str(f, parts) => {
                out.extend(f.iter().copied());
                for p in parts {
                    match p {
                        StrPart::Str(s) => out.push(s),
                        StrPart::Term(t) => strings(t, out),
                        StrPart::Char(_) => {}
                    }
                }
            }
            Term::Call(name, args) => {
                out.push(name);
                for a in args {
                    strings(a, out);
                }
            }
            Term::Label(name, t) => {
                out.push(name);
                strings(t, out);
            }
            Term::Def(defs, t) => {
                for d in defs {
                    out.push(d.name);
                    out.extend(d.args.iter().copied());
                    strings(&d.body, out);
                }
                strings(t, out);
            }
            Term::Fold(keyword, xs, p, args) => {
                out.push(keyword);
                strings(xs, out);
                pattern(p, out);
                for a in args {
                    strings(a, out);
                }
            }
            Term::BinOp(l, op, r) => {
                strings(l, out);
                if let jaq::jaq_core::load::parse::BinaryOp::Pipe(Some(p)) = op {
                    pattern(p, out);
                }
                strings(r, out);
            }
            Term::Path(base, path) => {
                strings(base, out);
                for (part, _) in &path.0 {
                    match part {
                        Part::Index(i) => strings(i, out),
                        Part::Range(from, to) => {
                            for t in from.iter().chain(to) {
                                strings(t, out);
                            }
                        }
                    }
                }
            }
            Term::Arr(Some(t)) | Term::Neg(t) => strings(t, out),
            Term::Obj(kvs) => {
                for (k, v) in kvs {
                    strings(k, out);
                    if let Some(v) = v {
                        strings(v, out);
                    }
                }
            }
            Term::TryCatch(t, c) => {
                strings(t, out);
                if let Some(c) = c {
                    strings(c, out);
                }
            }
            Term::IfThenElse(branches, e) => {
                for (c, t) in branches {
                    strings(c, out);
                    strings(t, out);
                }
                if let Some(e) = e {
                    strings(e, out);
                }
            }
            Term::Arr(None) | Term::Id | Term::Recurse => {}
        }
    }

    #[test]
    fn every_string_is_a_slice_of_the_program() {
        // The engine locates a name by where its slice points.
        let src = "def f($a; g): $a + g; label $l | reduce (.[] as [$x, {y: $z}] | $x) \
                   as $v (0; f(.; $v) + 1.5) | {a: @base64 \"q\\n\\(.)\"} | .k?.\"m\"[1:]";
        let module = convert(src).expect("converts");
        let mut all = Vec::new();
        strings(&module.body, &mut all);
        assert!(all.len() > 20, "{all:?}");
        let range = src.as_ptr() as usize..=src.as_ptr() as usize + src.len();
        for s in all {
            let at = s.as_ptr() as usize;
            assert!(
                range.contains(&at) && range.contains(&(at + s.len())),
                "{s:?}"
            );
        }
    }
}
