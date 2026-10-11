// SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The jq parser, following the rules and precedence table of jq 1.8.1's
//! `src/parser.y`.
//!
//! The Query level is recursive descent; the Expr level is precedence climbing
//! over jq's operator table, lowest first:
//!
//! | level | operators | associativity |
//! |---|---|---|
//! | 1 | `//` | right |
//! | 2 | `=` `\|=` `+=` `-=` `*=` `/=` `%=` `//=` | none |
//! | 3 | `or` | left |
//! | 4 | `and` | left |
//! | 5 | `==` `!=` `<` `<=` `>` `>=` | none |
//! | 6 | `+` `-` | left |
//! | 7 | `*` `/` `%` | left |
//!
//! `//` binding *looser* than `=` is jq's choice and is easy to get backwards:
//! `.a = 1 // 2` is `(.a = 1) // 2`.
//!
//! Three constructs extend to the end of the enclosing query and therefore
//! swallow any `|` or `,` after them: `def …;`, `label $x |`, and
//! `expr as $p |`. Postfix chains (`.a[0]?`) are greedy everywhere, and a
//! `try` body is a single postfix term: `try .a + 1` is `(try .a) + 1`.

use super::lex::{Kw, StrPart, Tok, Token, lex};
use super::{
    FieldName, FuncDef, Kind, Node, ObjKey, ObjPair, ObjPat, ObjShort, Param, Pattern, Span,
    StrLit, StrSeg,
};

/// Input the parser does not accept. jq rejects most of it too; the rest is
/// passed to jaq unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub at: usize,
    pub what: String,
}

type Result<T> = std::result::Result<T, ParseError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Assoc {
    Left,
    Right,
    None,
}

/// Parse a program body: no `module`/`import`/`include` header.
pub fn parse(src: &str) -> Result<Node> {
    let toks = lex(src).map_err(|e| ParseError {
        at: e.0,
        what: "invalid token".to_owned(),
    })?;
    let mut p = Parser { toks, pos: 0 };
    if matches!(
        p.peek(),
        Tok::Keyword(Kw::Module | Kw::Import | Kw::Include)
    ) {
        return Err(p.error("module header"));
    }
    let node = p.query()?;
    p.expect_eof()?;
    Ok(node)
}

struct Parser {
    toks: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> &Tok {
        &self.toks[self.pos].tok
    }

    fn peek_at(&self, n: usize) -> &Tok {
        &self.toks[(self.pos + n).min(self.toks.len() - 1)].tok
    }

    fn here(&self) -> usize {
        self.toks[self.pos].span.start
    }

    /// End of the most recently consumed token.
    fn last_end(&self) -> usize {
        self.toks[self.pos.saturating_sub(1)].span.end
    }

    fn bump(&mut self) -> Token {
        let t = self.toks[self.pos].clone();
        if self.pos < self.toks.len() - 1 {
            self.pos += 1;
        }
        t
    }

    fn error(&self, what: &str) -> ParseError {
        ParseError {
            at: self.here(),
            what: what.to_owned(),
        }
    }

    fn is_op(&self, op: &str) -> bool {
        matches!(self.peek(), Tok::Op(o) if *o == op)
    }

    fn is_kw(&self, kw: Kw) -> bool {
        matches!(self.peek(), Tok::Keyword(k) if *k == kw)
    }

    fn eat_op(&mut self, op: &str) -> bool {
        if self.is_op(op) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn expect_op(&mut self, op: &str) -> Result<()> {
        if self.eat_op(op) {
            Ok(())
        } else {
            Err(self.error(&format!("expected `{op}`")))
        }
    }

    fn expect_kw(&mut self, kw: Kw) -> Result<()> {
        if self.is_kw(kw) {
            self.bump();
            Ok(())
        } else {
            Err(self.error(&format!("expected `{}`", kw.as_str())))
        }
    }

    fn expect_eof(&self) -> Result<()> {
        if matches!(self.peek(), Tok::Eof) {
            Ok(())
        } else {
            Err(self.error("unexpected token"))
        }
    }

    fn binding(&mut self) -> Result<String> {
        match self.peek().clone() {
            Tok::Binding(name) => {
                self.bump();
                Ok(name)
            }
            _ => Err(self.error("expected `$name`")),
        }
    }

    // --- Query --------------------------------------------------------------

    /// `Query: Query '|' Query` (right-associative, lowest).
    fn query(&mut self) -> Result<Node> {
        let start = self.here();
        let lhs = self.comma()?;
        if self.eat_op("|") {
            let rhs = self.query()?;
            return Ok(Node::new(
                Span::new(start, rhs.span.end),
                Kind::Pipe(Box::new(lhs), Box::new(rhs)),
            ));
        }
        Ok(lhs)
    }

    /// `Query: Query ',' Query` (left-associative).
    fn comma(&mut self) -> Result<Node> {
        let start = self.here();
        let mut lhs = self.operand()?;
        while self.eat_op(",") {
            let rhs = self.operand()?;
            lhs = Node::new(
                Span::new(start, rhs.span.end),
                Kind::Comma(Box::new(lhs), Box::new(rhs)),
            );
        }
        Ok(lhs)
    }

    /// An operand of `|` or `,`: a definition, a label, a binding, or an Expr.
    /// The first three swallow the rest of the query.
    fn operand(&mut self) -> Result<Node> {
        let start = self.here();
        if self.is_kw(Kw::Def) {
            let def = self.funcdef()?;
            let rest = self.query()?;
            return Ok(Node::new(
                Span::new(start, rest.span.end),
                Kind::Def {
                    def: Box::new(def),
                    rest: Box::new(rest),
                },
            ));
        }
        if self.is_kw(Kw::Label) {
            self.bump();
            let name = self.binding()?;
            self.expect_op("|")?;
            let body = self.query()?;
            return Ok(Node::new(
                Span::new(start, body.span.end),
                Kind::Label {
                    name,
                    body: Box::new(body),
                },
            ));
        }
        let source = self.expr(1)?;
        if self.is_kw(Kw::As) {
            self.bump();
            let patterns = self.patterns()?;
            self.expect_op("|")?;
            let body = self.query()?;
            return Ok(Node::new(
                Span::new(start, body.span.end),
                Kind::Bind {
                    source: Box::new(source),
                    patterns,
                    body: Box::new(body),
                },
            ));
        }
        Ok(source)
    }

    fn funcdef(&mut self) -> Result<FuncDef> {
        self.expect_kw(Kw::Def)?;
        let name = match self.peek().clone() {
            Tok::Ident(n) => {
                self.bump();
                n
            }
            _ => return Err(self.error("expected a function name")),
        };
        let mut params = Vec::new();
        if self.eat_op("(") {
            loop {
                match self.peek().clone() {
                    Tok::Ident(n) => params.push(Param::Filter(n)),
                    Tok::Binding(n) => params.push(Param::Value(n)),
                    _ => return Err(self.error("expected a parameter")),
                }
                self.bump();
                if !self.eat_op(";") {
                    break;
                }
            }
            self.expect_op(")")?;
        }
        self.expect_op(":")?;
        let body = self.query()?;
        self.expect_op(";")?;
        Ok(FuncDef { name, params, body })
    }

    // --- Expr ---------------------------------------------------------------

    fn binop(&self) -> Option<(&'static str, u8, Assoc)> {
        Some(match self.peek() {
            Tok::Op("//") => ("//", 1, Assoc::Right),
            Tok::Op(o @ ("=" | "|=" | "+=" | "-=" | "*=" | "/=" | "%=" | "//=")) => {
                (o, 2, Assoc::None)
            }
            Tok::Keyword(Kw::Or) => ("or", 3, Assoc::Left),
            Tok::Keyword(Kw::And) => ("and", 4, Assoc::Left),
            Tok::Op(o @ ("==" | "!=" | "<" | "<=" | ">" | ">=")) => (o, 5, Assoc::None),
            Tok::Op(o @ ("+" | "-")) => (o, 6, Assoc::Left),
            Tok::Op(o @ ("*" | "/" | "%")) => (o, 7, Assoc::Left),
            _ => return None,
        })
    }

    /// Precedence climbing over jq's operator table.
    fn expr(&mut self, min: u8) -> Result<Node> {
        let start = self.here();
        let mut lhs = self.term()?;
        while let Some((op, prec, assoc)) = self.binop() {
            if prec < min {
                break;
            }
            self.bump();
            let next = if assoc == Assoc::Right {
                prec
            } else {
                prec + 1
            };
            let rhs = self.expr(next)?;
            lhs = Node::new(
                Span::new(start, rhs.span.end),
                Kind::Binary {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
            );
            // jq's %nonassoc: `a == b == c` and `a = b = c` are syntax errors.
            if assoc == Assoc::None && self.binop().is_some_and(|(_, p, _)| p == prec) {
                return Err(self.error("non-associative operator chained"));
            }
        }
        Ok(lhs)
    }

    // --- Term ---------------------------------------------------------------

    fn term(&mut self) -> Result<Node> {
        let mut node = self.primary()?;
        loop {
            let start = node.span.start;
            match self.peek().clone() {
                Tok::Field(name) => {
                    self.bump();
                    node = Node::new(
                        Span::new(start, self.last_end()),
                        Kind::Field {
                            base: Box::new(node),
                            name: FieldName::Ident(name),
                        },
                    );
                }
                Tok::Op(".") if matches!(self.peek_at(1), Tok::Str(_)) => {
                    self.bump();
                    let s = self.string(None)?;
                    node = Node::new(
                        Span::new(start, self.last_end()),
                        Kind::Field {
                            base: Box::new(node),
                            name: FieldName::Str(s),
                        },
                    );
                }
                Tok::Op(".") if matches!(self.peek_at(1), Tok::Op("[")) => {
                    self.bump();
                    node = self.bracket(node)?;
                }
                Tok::Op("[") => node = self.bracket(node)?,
                Tok::Op("?") => {
                    self.bump();
                    node = Node::new(
                        Span::new(start, self.last_end()),
                        Kind::Optional(Box::new(node)),
                    );
                }
                _ => break,
            }
        }
        Ok(node)
    }

    /// `[…]` after a term: iterate, index, or slice.
    fn bracket(&mut self, base: Node) -> Result<Node> {
        let start = base.span.start;
        self.expect_op("[")?;
        let kind = if self.eat_op("]") {
            Kind::Iterate(Box::new(base))
        } else if self.eat_op(":") {
            let to = self.query()?;
            self.expect_op("]")?;
            Kind::Slice {
                base: Box::new(base),
                from: None,
                to: Some(Box::new(to)),
            }
        } else {
            let first = self.query()?;
            if self.eat_op(":") {
                let to = if self.is_op("]") {
                    None
                } else {
                    Some(Box::new(self.query()?))
                };
                self.expect_op("]")?;
                Kind::Slice {
                    base: Box::new(base),
                    from: Some(Box::new(first)),
                    to,
                }
            } else {
                self.expect_op("]")?;
                Kind::Index {
                    base: Box::new(base),
                    index: Box::new(first),
                }
            }
        };
        Ok(Node::new(Span::new(start, self.last_end()), kind))
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one arm per parser.y Term production; splitting would hide the correspondence"
    )]
    fn primary(&mut self) -> Result<Node> {
        let start = self.here();
        let tok = self.peek().clone();
        let kind = match tok {
            Tok::Op(".") => {
                self.bump();
                if matches!(self.peek(), Tok::Str(_)) {
                    let s = self.string(None)?;
                    Kind::Field {
                        base: Box::new(Node::new(Span::empty(start), Kind::Identity)),
                        name: FieldName::Str(s),
                    }
                } else {
                    Kind::Identity
                }
            }
            Tok::Field(name) => {
                self.bump();
                Kind::Field {
                    base: Box::new(Node::new(Span::empty(start), Kind::Identity)),
                    name: FieldName::Ident(name),
                }
            }
            Tok::Op("..") => {
                self.bump();
                Kind::Recurse
            }
            Tok::Number => {
                self.bump();
                Kind::Number
            }
            Tok::Str(_) => Kind::Str(self.string(None)?),
            Tok::Format(f) => {
                self.bump();
                if matches!(self.peek(), Tok::Str(_)) {
                    Kind::Str(self.string(Some(f))?)
                } else {
                    Kind::Format(f)
                }
            }
            Tok::Op("-") => {
                self.bump();
                Kind::Neg(Box::new(self.term()?))
            }
            Tok::Op("(") => {
                self.bump();
                let q = self.query()?;
                self.expect_op(")")?;
                Kind::Paren(Box::new(q))
            }
            Tok::Op("[") => {
                self.bump();
                if self.eat_op("]") {
                    Kind::Array(None)
                } else {
                    let q = self.query()?;
                    self.expect_op("]")?;
                    Kind::Array(Some(Box::new(q)))
                }
            }
            Tok::Op("{") => {
                self.bump();
                Kind::Object(self.object()?)
            }
            Tok::Keyword(Kw::Reduce) => self.reduce()?,
            Tok::Keyword(Kw::Foreach) => self.foreach()?,
            Tok::Keyword(Kw::If) => self.if_()?,
            Tok::Keyword(Kw::Try) => {
                self.bump();
                let body = self.term()?;
                let handler = if self.is_kw(Kw::Catch) {
                    self.bump();
                    Some(Box::new(self.term()?))
                } else {
                    None
                };
                Kind::Try {
                    body: Box::new(body),
                    handler,
                }
            }
            Tok::Keyword(Kw::Break) => {
                self.bump();
                Kind::Break(self.binding()?)
            }
            Tok::Binding(name) => {
                self.bump();
                Kind::Var(name)
            }
            Tok::Loc => {
                self.bump();
                Kind::Loc
            }
            Tok::Ident(name) => {
                self.bump();
                let mut args = Vec::new();
                if self.eat_op("(") {
                    loop {
                        args.push(self.query()?);
                        if !self.eat_op(";") {
                            break;
                        }
                    }
                    self.expect_op(")")?;
                }
                Kind::Call { name, args }
            }
            _ => return Err(self.error("expected an expression")),
        };
        Ok(Node::new(Span::new(start, self.last_end()), kind))
    }

    fn reduce(&mut self) -> Result<Kind> {
        self.expect_kw(Kw::Reduce)?;
        let source = self.expr(1)?;
        self.expect_kw(Kw::As)?;
        let patterns = self.patterns()?;
        self.expect_op("(")?;
        let init = self.query()?;
        self.expect_op(";")?;
        let update = self.query()?;
        self.expect_op(")")?;
        Ok(Kind::Reduce {
            source: Box::new(source),
            patterns,
            init: Box::new(init),
            update: Box::new(update),
        })
    }

    fn foreach(&mut self) -> Result<Kind> {
        self.expect_kw(Kw::Foreach)?;
        let source = self.expr(1)?;
        self.expect_kw(Kw::As)?;
        let patterns = self.patterns()?;
        self.expect_op("(")?;
        let init = self.query()?;
        self.expect_op(";")?;
        let update = self.query()?;
        let extract = if self.eat_op(";") {
            Some(Box::new(self.query()?))
        } else {
            None
        };
        self.expect_op(")")?;
        Ok(Kind::Foreach {
            source: Box::new(source),
            patterns,
            init: Box::new(init),
            update: Box::new(update),
            extract,
        })
    }

    fn if_(&mut self) -> Result<Kind> {
        self.expect_kw(Kw::If)?;
        let mut branches = Vec::new();
        let cond = self.query()?;
        self.expect_kw(Kw::Then)?;
        let then = self.query()?;
        branches.push((cond, then));
        loop {
            if self.is_kw(Kw::Elif) {
                self.bump();
                let c = self.query()?;
                self.expect_kw(Kw::Then)?;
                let t = self.query()?;
                branches.push((c, t));
            } else if self.is_kw(Kw::Else) {
                self.bump();
                let e = self.query()?;
                self.expect_kw(Kw::End)?;
                return Ok(Kind::If {
                    branches,
                    otherwise: Some(Box::new(e)),
                });
            } else {
                self.expect_kw(Kw::End)?;
                return Ok(Kind::If {
                    branches,
                    otherwise: None,
                });
            }
        }
    }

    // --- Strings --------------------------------------------------------------

    fn string(&mut self, format: Option<String>) -> Result<StrLit> {
        let Tok::Str(parts) = self.bump().tok else {
            return Err(self.error("expected a string"));
        };
        let mut segs = Vec::with_capacity(parts.len());
        for part in parts {
            match part {
                StrPart::Text(span) => segs.push(StrSeg::Text(span)),
                StrPart::Interp(mut toks, span) => {
                    let end = span.end;
                    toks.push(Token {
                        tok: Tok::Eof,
                        span: Span::empty(end),
                    });
                    let mut sub = Self { toks, pos: 0 };
                    let q = sub.query()?;
                    sub.expect_eof()?;
                    segs.push(StrSeg::Interp(Box::new(q)));
                }
            }
        }
        Ok(StrLit {
            format,
            parts: segs,
        })
    }

    /// A string token, possibly preceded by a format: `"a"` or `@base64 "a"`.
    fn maybe_string(&mut self) -> Result<Option<StrLit>> {
        match self.peek().clone() {
            Tok::Str(_) => Ok(Some(self.string(None)?)),
            Tok::Format(f) if matches!(self.peek_at(1), Tok::Str(_)) => {
                self.bump();
                Ok(Some(self.string(Some(f))?))
            }
            _ => Ok(None),
        }
    }

    // --- Objects --------------------------------------------------------------

    fn object(&mut self) -> Result<Vec<ObjPair>> {
        let mut pairs = Vec::new();
        if self.eat_op("}") {
            return Ok(pairs);
        }
        loop {
            pairs.push(self.pair()?);
            if self.eat_op("}") {
                return Ok(pairs);
            }
            self.expect_op(",")?;
            // jq's `DictPairs` may be empty after a comma, so `{a: 1,}` is legal
            // (but `{,}` is not: the first pair is required).
            if self.eat_op("}") {
                return Ok(pairs);
            }
        }
    }

    fn pair(&mut self) -> Result<ObjPair> {
        if let Some(s) = self.maybe_string()? {
            if self.eat_op(":") {
                let value = self.dict_expr()?;
                return Ok(ObjPair::KeyValue {
                    key: ObjKey::Str(s),
                    value,
                });
            }
            return Ok(ObjPair::Shorthand(ObjShort::Str(s)));
        }
        match self.peek().clone() {
            Tok::Ident(name) => {
                self.bump();
                if self.eat_op(":") {
                    let value = self.dict_expr()?;
                    return Ok(ObjPair::KeyValue {
                        key: ObjKey::Ident(name),
                        value,
                    });
                }
                Ok(ObjPair::Shorthand(ObjShort::Ident(name)))
            }
            Tok::Keyword(k) => {
                self.bump();
                let name = k.as_str().to_owned();
                if self.eat_op(":") {
                    let value = self.dict_expr()?;
                    return Ok(ObjPair::KeyValue {
                        key: ObjKey::Ident(name),
                        value,
                    });
                }
                Ok(ObjPair::Shorthand(ObjShort::Ident(name)))
            }
            Tok::Binding(name) => {
                self.bump();
                if self.eat_op(":") {
                    let value = self.dict_expr()?;
                    return Ok(ObjPair::KeyValue {
                        key: ObjKey::Var(name),
                        value,
                    });
                }
                Ok(ObjPair::Shorthand(ObjShort::Var(name)))
            }
            Tok::Loc => {
                let span = self.bump().span;
                Ok(ObjPair::Shorthand(ObjShort::Loc(span)))
            }
            Tok::Op("(") => {
                let start = self.here();
                self.bump();
                let q = self.query()?;
                self.expect_op(")")?;
                let key = Node::new(Span::new(start, self.last_end()), Kind::Paren(Box::new(q)));
                self.expect_op(":")?;
                let value = self.dict_expr()?;
                Ok(ObjPair::KeyValue {
                    key: ObjKey::Expr(Box::new(key)),
                    value,
                })
            }
            _ => Err(self.error("expected an object key")),
        }
    }

    /// `DictExpr: DictExpr '|' DictExpr | Expr` — pipes allowed, commas not.
    fn dict_expr(&mut self) -> Result<Node> {
        let start = self.here();
        let lhs = self.expr(1)?;
        if self.eat_op("|") {
            let rhs = self.dict_expr()?;
            return Ok(Node::new(
                Span::new(start, rhs.span.end),
                Kind::Pipe(Box::new(lhs), Box::new(rhs)),
            ));
        }
        Ok(lhs)
    }

    // --- Patterns -------------------------------------------------------------

    /// `Patterns: Pattern ('?//' Pattern)*`
    fn patterns(&mut self) -> Result<Vec<Pattern>> {
        let mut out = vec![self.pattern()?];
        while self.eat_op("?//") {
            out.push(self.pattern()?);
        }
        Ok(out)
    }

    fn pattern(&mut self) -> Result<Pattern> {
        match self.peek().clone() {
            Tok::Binding(name) => {
                self.bump();
                Ok(Pattern::Var(name))
            }
            Tok::Op("[") => {
                self.bump();
                let mut items = vec![self.pattern()?];
                while self.eat_op(",") {
                    items.push(self.pattern()?);
                }
                self.expect_op("]")?;
                Ok(Pattern::Array(items))
            }
            Tok::Op("{") => {
                self.bump();
                let mut items = vec![self.obj_pattern()?];
                while self.eat_op(",") {
                    items.push(self.obj_pattern()?);
                }
                self.expect_op("}")?;
                Ok(Pattern::Object(items))
            }
            _ => Err(self.error("expected a pattern")),
        }
    }

    fn obj_pattern(&mut self) -> Result<ObjPat> {
        if let Some(s) = self.maybe_string()? {
            self.expect_op(":")?;
            return Ok(ObjPat::Key(ObjKey::Str(s), self.pattern()?));
        }
        match self.peek().clone() {
            Tok::Binding(name) => {
                self.bump();
                if self.eat_op(":") {
                    return Ok(ObjPat::VarPattern(name, self.pattern()?));
                }
                Ok(ObjPat::Var(name))
            }
            Tok::Ident(name) => {
                self.bump();
                self.expect_op(":")?;
                Ok(ObjPat::Key(ObjKey::Ident(name), self.pattern()?))
            }
            Tok::Keyword(k) => {
                self.bump();
                self.expect_op(":")?;
                Ok(ObjPat::Key(
                    ObjKey::Ident(k.as_str().to_owned()),
                    self.pattern()?,
                ))
            }
            Tok::Op("(") => {
                let start = self.here();
                self.bump();
                let q = self.query()?;
                self.expect_op(")")?;
                let key = Node::new(Span::new(start, self.last_end()), Kind::Paren(Box::new(q)));
                self.expect_op(":")?;
                Ok(ObjPat::Key(ObjKey::Expr(Box::new(key)), self.pattern()?))
            }
            _ => Err(self.error("expected an object pattern")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse;
    use crate::syntax::{Kind, Node};

    /// Render the tree as an S-expression of operators, to assert on grouping.
    fn shape(src: &str) -> String {
        fn go(n: &Node, src: &str) -> String {
            match &n.kind {
                Kind::Pipe(a, b) => format!("(| {} {})", go(a, src), go(b, src)),
                Kind::Comma(a, b) => format!("(, {} {})", go(a, src), go(b, src)),
                Kind::Binary { op, lhs, rhs } => {
                    format!("({op} {} {})", go(lhs, src), go(rhs, src))
                }
                Kind::Bind { source, body, .. } => {
                    format!("(as {} {})", go(source, src), go(body, src))
                }
                Kind::Def { rest, .. } => format!("(def {})", go(rest, src)),
                Kind::Try { body, handler } => match handler {
                    Some(h) => format!("(try {} {})", go(body, src), go(h, src)),
                    None => format!("(try {})", go(body, src)),
                },
                Kind::Reduce { source, .. } => format!("(reduce {})", go(source, src)),
                Kind::Neg(t) => format!("(neg {})", go(t, src)),
                _ => src[n.span.start..n.span.end].to_owned(),
            }
        }
        go(&parse(src).expect("parses"), src)
    }

    #[test]
    fn alternative_binds_looser_than_assignment() {
        // jq's table: `//` is below `=`, so this is `(.a = 1) // 2`.
        assert_eq!(shape(".a = 1 // 2"), "(// (= .a 1) 2)");
        assert_eq!(
            shape("a // b // c"),
            "(// a (// b c))",
            "`//` is right-associative"
        );
    }

    #[test]
    fn arithmetic_precedence_and_associativity() {
        assert_eq!(shape("1 + 2 * 3"), "(+ 1 (* 2 3))");
        assert_eq!(shape("1 - 2 - 3"), "(- (- 1 2) 3)");
        assert_eq!(shape("a or b and c"), "(or a (and b c))");
        assert_eq!(shape(".a += 1 | .b"), "(| (+= .a 1) .b)");
    }

    #[test]
    fn non_associative_chains_are_rejected() {
        parse("1 == 2 == 3").unwrap_err();
        parse(".a = 1 = 2").unwrap_err();
    }

    #[test]
    fn pipe_is_lowest_and_right_associative_comma_binds_tighter() {
        assert_eq!(shape("a | b | c"), "(| a (| b c))");
        assert_eq!(shape("a, b | c"), "(| (, a b) c)");
    }

    #[test]
    fn bindings_and_defs_swallow_the_rest_of_the_query() {
        assert_eq!(shape(". as $x | $x, 1"), "(as . (, $x 1))");
        assert_eq!(shape("a, b as $x | c"), "(, a (as b c))");
        assert_eq!(shape("def f: 1; f, 2"), "(def (, f 2))");
        assert_eq!(shape("1 + 2 as $x | $x"), "(as (+ 1 2) $x)");
    }

    #[test]
    fn reduce_source_is_a_whole_expression() {
        // jaq rejects this form; jq parses the division as the source.
        assert_eq!(
            shape("reduce .[] / .[] as $i (0; . + $i)"),
            "(reduce (/ .[] .[]))"
        );
    }

    #[test]
    fn try_body_is_one_postfix_term() {
        assert_eq!(shape("try .a + 1"), "(+ (try .a) 1)");
        assert_eq!(shape("try .a.b[0]? catch . | f"), "(| (try .a.b[0]? .) f)");
        assert!(parse("try 1 + 2 catch .").is_err(), "jq rejects this too");
    }

    #[test]
    fn unary_minus_applies_to_a_term() {
        assert_eq!(shape("-1 + 2"), "(+ (neg 1) 2)");
        assert_eq!(shape("-.a[0]"), "(neg .a[0])");
    }

    #[test]
    fn postfix_chains_and_slices() {
        for src in [
            ".a.b[0]",
            ".a.[0]",
            ".[1:]",
            ".[:2]",
            ".[1:2]?",
            ".a?.b",
            "..[]?",
            ".\"x y\".z",
            "$x.a",
        ] {
            assert!(parse(src).is_ok(), "{src}");
        }
    }

    #[test]
    fn objects_strings_and_patterns() {
        for src in [
            "{a, $x, \"k\", (1,2): 3, if: 4, @base64 \"s\": 5, $k: 6, a: 1 | . + 1}",
            "\"a\\(1 + 2)b\\(\"nested \\(3)\")\"",
            ". as {$a, b: [$c, {$d}], \"e\": $e, (\"f\"): $f, $g: [$h]} ?// [$i] | 1",
            "reduce .[] as [$a, $b] (0; . + $a)",
            "foreach .[] as $x (0; . + $x; [$x, .])",
            "if . then 1 elif 2 then 3 end",
            "label $out | 1, break $out",
            "def f($a; g): $a | g; f(1; . + 1)",
            "{$__loc__}",
        ] {
            assert!(parse(src).is_ok(), "{src}: {:?}", parse(src).err());
        }
    }

    #[test]
    fn jq_syntax_errors_stay_errors() {
        for src in [
            ".a?//1",
            "{1: 2}",
            "{,}",
            "(1",
            "[1,]",
            "if . then 1",
            ".a as $x",
            "f(1;)",
            ". as {a: $x,} | $x",
        ] {
            assert!(parse(src).is_err(), "{src} should not parse");
        }
    }

    #[test]
    fn a_trailing_comma_is_legal_in_objects_only() {
        assert!(parse("{a: 1,}").is_ok(), "jq 1.8.1 accepts this");
    }

    #[test]
    fn every_valid_program_in_jqs_own_suite_parses() {
        // The vendored jq 1.8.1 test files contain several hundred programs jq
        // accepts. The parser must accept every one: a rejection means a
        // program Pathfinder could never rewrite, and usually a grammar gap.
        // (`%%FAIL` blocks are programs jq rejects, so they are skipped here
        // and handled by the conformance suite.) Bodies only, after the
        // `import`/`include` header is split off, exactly as `program.rs` does.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/jq-suite");
        let mut rejected = Vec::new();
        let mut total = 0usize;
        let mut files: Vec<_> = std::fs::read_dir(&dir)
            .expect("suite dir")
            .filter_map(std::result::Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "test"))
            .collect();
        files.sort();
        for f in files {
            let text = std::fs::read_to_string(&f).expect("readable");
            let lines: Vec<&str> = text.split('\n').collect();
            let mut i = 0usize;
            while i < lines.len() {
                let l = lines[i];
                if l.trim().is_empty() || l.starts_with('#') {
                    i += 1;
                    continue;
                }
                let fail_block = l.starts_with("%%FAIL");
                if fail_block {
                    i += 1;
                }
                let program = lines[i];
                let line_no = i + 1;
                i += 1;
                while i < lines.len() && !lines[i].trim().is_empty() {
                    i += 1;
                }
                if fail_block {
                    continue;
                }
                total += 1;
                // As in production: the module header is split off first and
                // only the body reaches the parser.
                let body = &program[crate::scan::scan(program).header_end..];
                if let Err(e) = parse(body) {
                    let name = f
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned();
                    rejected.push(format!("{name}:{line_no} {program}  ({e:?})"));
                }
            }
        }
        assert!(total > 700, "suite not found or empty: {total} programs");
        assert!(
            rejected.is_empty(),
            "{} of {total} valid programs rejected:\n{}",
            rejected.len(),
            rejected.join("\n")
        );
    }

    #[test]
    fn spans_cover_their_source() {
        let src = "[.a = 1, 2]";
        let n = parse(src).expect("parses");
        assert_eq!((n.span.start, n.span.end), (0, src.len()));
    }
}
