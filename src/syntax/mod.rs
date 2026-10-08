// SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A parser for the jq language, following jq 1.8.1's own grammar.
//!
//! Pathfinder needs one for the rewrites a definition cannot express:
//! assignment operators that must create missing containers, syntax jaq
//! parses differently or not at all, and programs jq rejects at compile time.
//!
//! # The safety rules this module exists under
//!
//! - **The grammar is jq's, not a guess at it.** `parse.rs` mirrors the rules
//!   and precedence table in `src/parser.y` and `lex.rs` mirrors `src/lexer.l`.
//!   A parser that disagrees with jq about where an expression ends rewrites a
//!   working program into a different one.
//! - **Untouched text is never reprinted.** Every node records its span, and the
//!   rewriter splices only the nodes it changes; everything else keeps the bytes
//!   the user wrote.
//! - **When in doubt, pass through.** A program this parser cannot handle is
//!   handed to jaq unchanged — exactly what happened before the parser existed.
//!
//! [`print::full`] is the exception to the second rule and is used only to test
//! the first: it reprints a program fully parenthesised from the tree, so a
//! wrong precedence decision changes the program's output and the conformance
//! suite sees it.

pub mod check;
pub mod lex;
pub mod parse;
pub mod print;
pub mod rewrite;

/// A byte range in the program text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub const fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    /// A zero-width span, for nodes the source does not spell out (the `.`
    /// implied by `.foo`).
    pub const fn empty(at: usize) -> Self {
        Self { start: at, end: at }
    }
}

/// A parsed expression.
#[derive(Debug, Clone)]
pub struct Node {
    pub span: Span,
    pub kind: Kind,
}

/// The shape of an expression, one variant per jq grammar construct.
#[derive(Debug, Clone)]
pub enum Kind {
    // --- Query level --------------------------------------------------------
    Pipe(Box<Node>, Box<Node>),
    Comma(Box<Node>, Box<Node>),
    /// `source as $p ?// $q | body`
    Bind {
        source: Box<Node>,
        patterns: Vec<Pattern>,
        body: Box<Node>,
    },
    Label {
        name: String,
        body: Box<Node>,
    },
    /// `def f: …; rest`
    Def {
        def: Box<FuncDef>,
        rest: Box<Node>,
    },
    // --- Expr level ---------------------------------------------------------
    /// Every binary operator, assignment forms included; `op` is its spelling.
    Binary {
        op: &'static str,
        lhs: Box<Node>,
        rhs: Box<Node>,
    },
    /// `-term`
    Neg(Box<Node>),
    // --- Terms --------------------------------------------------------------
    Identity,
    Recurse,
    Number,
    Str(StrLit),
    /// A bare format, `@base64`.
    Format(String),
    Var(String),
    Loc,
    Break(String),
    Call {
        name: String,
        args: Vec<Node>,
    },
    Paren(Box<Node>),
    Array(Option<Box<Node>>),
    Object(Vec<ObjPair>),
    Reduce {
        source: Box<Node>,
        patterns: Vec<Pattern>,
        init: Box<Node>,
        update: Box<Node>,
    },
    Foreach {
        source: Box<Node>,
        patterns: Vec<Pattern>,
        init: Box<Node>,
        update: Box<Node>,
        extract: Option<Box<Node>>,
    },
    /// `if c then a elif c2 then b … else e end`; `otherwise` is `None` for the
    /// jq 1.7+ form without `else`, which means `.`.
    If {
        branches: Vec<(Node, Node)>,
        otherwise: Option<Box<Node>>,
    },
    Try {
        body: Box<Node>,
        handler: Option<Box<Node>>,
    },
    // --- Postfix ------------------------------------------------------------
    Field {
        base: Box<Node>,
        name: FieldName,
    },
    Index {
        base: Box<Node>,
        index: Box<Node>,
    },
    Slice {
        base: Box<Node>,
        from: Option<Box<Node>>,
        to: Option<Box<Node>>,
    },
    Iterate(Box<Node>),
    /// `term?`
    Optional(Box<Node>),
}

/// The name in `.foo` or `."foo"`.
#[derive(Debug, Clone)]
pub enum FieldName {
    Ident(String),
    Str(StrLit),
}

/// A string literal, possibly formatted (`@base64 "…"`), with its segments.
#[derive(Debug, Clone)]
pub struct StrLit {
    pub format: Option<String>,
    pub parts: Vec<StrSeg>,
}

/// A segment of a string literal.
#[derive(Debug, Clone)]
pub enum StrSeg {
    /// Literal text as written, escapes included.
    Text(Span),
    /// `\(query)`.
    Interp(Box<Node>),
}

/// `def name(params): body;`
#[derive(Debug, Clone)]
pub struct FuncDef {
    pub name: String,
    pub params: Vec<Param>,
    pub body: Node,
}

/// A function parameter: a filter (`f`) or a value (`$v`).
#[derive(Debug, Clone)]
pub enum Param {
    Filter(String),
    Value(String),
}

/// One `key: value` or shorthand entry in an object construction.
#[derive(Debug, Clone)]
pub enum ObjPair {
    KeyValue {
        key: ObjKey,
        value: Node,
    },
    /// `{a}`, `{"a"}`, `{$a}`, `{$__loc__}`.
    Shorthand(ObjShort),
}

/// An object key as written.
#[derive(Debug, Clone)]
pub enum ObjKey {
    /// An identifier or keyword: `{a: …}`, `{if: …}`.
    Ident(String),
    Str(StrLit),
    /// `{$x: …}` — the key is `$x`'s value.
    Var(String),
    /// `{(q): …}`
    Expr(Box<Node>),
}

/// Shorthand object entries.
#[derive(Debug, Clone)]
pub enum ObjShort {
    Ident(String),
    Str(StrLit),
    Var(String),
    Loc(Span),
}

/// A destructuring pattern.
#[derive(Debug, Clone)]
pub enum Pattern {
    Var(String),
    Array(Vec<Pattern>),
    Object(Vec<ObjPat>),
}

/// One entry of an object pattern.
#[derive(Debug, Clone)]
pub enum ObjPat {
    /// `{$a}`
    Var(String),
    /// `{$a: pattern}` — binds `$a` to `.a` *and* destructures it.
    VarPattern(String, Pattern),
    /// `{key: pattern}`
    Key(ObjKey, Pattern),
}

impl Node {
    pub const fn new(span: Span, kind: Kind) -> Self {
        Self { span, kind }
    }

    /// Direct child nodes, in source order. Every node the rewriter could
    /// change appears here, including those inside strings, keys and patterns.
    pub fn children(&self) -> Vec<&Self> {
        let mut out: Vec<&Self> = Vec::new();
        match &self.kind {
            Kind::Pipe(a, b) | Kind::Comma(a, b) => out.extend([&**a, &**b]),
            Kind::Bind {
                source,
                patterns,
                body,
            } => {
                out.push(source);
                for p in patterns {
                    p.collect(&mut out);
                }
                out.push(body);
            }
            Kind::Label { body, .. } => out.push(body),
            Kind::Def { def, rest } => out.extend([&def.body, &**rest]),
            Kind::Binary { lhs, rhs, .. } => out.extend([&**lhs, &**rhs]),
            Kind::Neg(n) | Kind::Paren(n) | Kind::Iterate(n) | Kind::Optional(n) => out.push(n),
            Kind::Str(s) => s.collect(&mut out),
            Kind::Call { args, .. } => out.extend(args),
            Kind::Array(n) => out.extend(n.as_deref()),
            Kind::Object(pairs) => {
                for p in pairs {
                    match p {
                        ObjPair::KeyValue { key, value } => {
                            key.collect(&mut out);
                            out.push(value);
                        }
                        ObjPair::Shorthand(ObjShort::Str(s)) => s.collect(&mut out),
                        ObjPair::Shorthand(_) => {}
                    }
                }
            }
            Kind::Reduce {
                source,
                patterns,
                init,
                update,
            } => {
                out.push(source);
                for p in patterns {
                    p.collect(&mut out);
                }
                out.extend([&**init, &**update]);
            }
            Kind::Foreach {
                source,
                patterns,
                init,
                update,
                extract,
            } => {
                out.push(source);
                for p in patterns {
                    p.collect(&mut out);
                }
                out.extend([&**init, &**update]);
                out.extend(extract.as_deref());
            }
            Kind::If {
                branches,
                otherwise,
            } => {
                for (c, t) in branches {
                    out.extend([c, t]);
                }
                out.extend(otherwise.as_deref());
            }
            Kind::Try { body, handler } => {
                out.push(body);
                out.extend(handler.as_deref());
            }
            Kind::Field { base, name } => {
                out.push(base);
                if let FieldName::Str(s) = name {
                    s.collect(&mut out);
                }
            }
            Kind::Index { base, index } => out.extend([&**base, &**index]),
            Kind::Slice { base, from, to } => {
                out.push(base);
                out.extend(from.as_deref());
                out.extend(to.as_deref());
            }
            Kind::Identity
            | Kind::Recurse
            | Kind::Number
            | Kind::Format(_)
            | Kind::Var(_)
            | Kind::Loc
            | Kind::Break(_) => {}
        }
        out
    }
}

impl StrLit {
    fn collect<'a>(&'a self, out: &mut Vec<&'a Node>) {
        for seg in &self.parts {
            if let StrSeg::Interp(n) = seg {
                out.push(n);
            }
        }
    }
}

impl ObjKey {
    fn collect<'a>(&'a self, out: &mut Vec<&'a Node>) {
        match self {
            Self::Str(s) => s.collect(out),
            Self::Expr(n) => out.push(n),
            Self::Ident(_) | Self::Var(_) => {}
        }
    }
}

impl Pattern {
    fn collect<'a>(&'a self, out: &mut Vec<&'a Node>) {
        match self {
            Self::Var(_) => {}
            Self::Array(ps) => {
                for p in ps {
                    p.collect(out);
                }
            }
            Self::Object(ps) => {
                for p in ps {
                    match p {
                        ObjPat::Var(_) => {}
                        ObjPat::VarPattern(_, p) => p.collect(out),
                        ObjPat::Key(k, p) => {
                            k.collect(out);
                            p.collect(out);
                        }
                    }
                }
            }
        }
    }
}
