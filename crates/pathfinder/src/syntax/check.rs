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
//! - A call, in reachable code, to a function jq does not define: jq answers
//!   `isarray/0 is not defined`. The engine defines helpers of its own
//!   (`isarray`, `tobytes`, `halt/1`, …) that a filter could otherwise call
//!   (PRD R-004), and it words an unknown name its own way
//!   ([`undefined_calls`]).
//!
//! jq's folding is narrow, and so is this module's: literals, `$__loc__`,
//! arrays and objects of constants, parentheses, `c | .`, and arithmetic or
//! comparison between two constants that jq can evaluate. `and`, `or`, `//`,
//! negation, `as`, commas and calls are not folded by jq, so not here either.

use super::lex::{Kw, StrPart, Tok, Token, lex};
use super::parse::parse;
use super::{
    FieldName, FuncDef, Kind, Node, ObjKey, ObjPair, ObjPat, ObjShort, Param, Pattern, Span,
    StrLit, StrSeg,
};
use std::fmt::Write as _;
use std::rc::Rc;

/// A compile error, located in the program text it was found in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileError {
    pub message: String,
    pub span: Span,
    /// Further errors jq reports with this one, in its order.
    pub more: Vec<CompileError>,
}

impl CompileError {
    /// jq's `UNKNOWN_LOCATION`: an error reported without a place.
    pub const UNLOCATED: Span = Span::new(usize::MAX, usize::MAX);

    /// jq's rendering: the message with its location, the source line, and a
    /// caret under the offending text.
    pub fn render(&self, prog: &str, src: &str) -> String {
        let mut out = self.render_one(prog, src);
        for e in &self.more {
            out.push_str(&e.render_one(prog, src));
        }
        let n = 1 + self.more.len();
        let plural = if n == 1 { "" } else { "s" };
        // Writing into a `String` cannot fail.
        let _ = writeln!(out, "{prog}: {n} compile error{plural}");
        out
    }

    /// One error's lines, without jq's closing count, as jq's
    /// `locfile_locate` writes them: the column counts bytes, and the carets
    /// run to the span's end on its first line, at least one past its start.
    fn render_one(&self, prog: &str, src: &str) -> String {
        if self.span == Self::UNLOCATED {
            return format!("{prog}: error: {}\n", self.message);
        }
        let b = src.as_bytes();
        let start = self.span.start.min(b.len());
        let line_start = b[..start]
            .iter()
            .rposition(|&c| c == b'\n')
            .map_or(0, |i| i + 1);
        let line_end = b[start..]
            .iter()
            .position(|&c| c == b'\n')
            .map_or(b.len(), |i| start + i);
        let line = b[..start].split(|&c| c == b'\n').count();
        let end = self.span.end.min(line_end.max(start + 1));
        let width = end.saturating_sub(start);
        format!(
            "{prog}: error: {} at <top-level>, line {line}, column {}:\n    {}\n    {}{}\n",
            self.message,
            start - line_start + 1,
            String::from_utf8_lossy(&b[line_start..line_end]),
            " ".repeat(start - line_start),
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
                more: Vec::new(),
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
        more: Vec::new(),
    })
}

/// The internal builtins jq 1.8.1 accepts in a program though `builtins`
/// leaves them out: every `_` name in jq's binary and `builtin.jq`, kept
/// where `jq -n 'if false then NAME(args) else 1 end'` compiles.
const JQ_INTERNAL: &[(&str, usize)] = &[
    ("_assign", 2),
    ("_divide", 2),
    ("_equal", 2),
    ("_flatten", 1),
    ("_greater", 2),
    ("_greatereq", 2),
    ("_group_by_impl", 1),
    ("_less", 2),
    ("_lesseq", 2),
    ("_match_impl", 3),
    ("_max_by_impl", 1),
    ("_min_by_impl", 1),
    ("_minus", 2),
    ("_mod", 2),
    ("_modify", 2),
    ("_multiply", 2),
    ("_negate", 0),
    ("_notequal", 2),
    ("_plus", 2),
    ("_sort_by_impl", 1),
    ("_strindices", 1),
    ("_unique_by_impl", 1),
];

/// Whether jq 1.8.1 defines `name/arity` without the program's help: one of
/// its `builtins` (`prelude::JQ_BUILTINS`, captured from jq), one of its
/// internal `_` builtins ([`JQ_INTERNAL`]), or a literal. The engine's own
/// `_` helpers (`_isarray`, `_tobytes`, …) are none of these.
fn defined_by_jq(name: &str, arity: usize) -> bool {
    // A set: every call and every identifier token asks.
    static BUILTINS: std::sync::OnceLock<std::collections::HashSet<(&'static str, usize)>> =
        std::sync::OnceLock::new();
    let builtins = BUILTINS.get_or_init(|| {
        crate::prelude::JQ_BUILTINS
            .split('"')
            .filter_map(|entry| {
                let (n, a) = entry.rsplit_once('/')?;
                Some((n, a.parse().ok()?))
            })
            .collect()
    });
    builtins.contains(&(name, arity))
        || JQ_INTERNAL.contains(&(name, arity))
        || (arity == 0 && matches!(name, "true" | "false" | "null"))
}

/// Whether the tokens may call a name jq does not define: the cheap test
/// before [`undefined_calls`] parses. Each identifier is taken as a call
/// with the arguments the tokens after it give it, counted as the `;`s at
/// the depth of a `(` that follows it. Identifiers that are no calls (object
/// keys, `def` names) can only make it parse when it need not; a program
/// calling only builtins, the common case, is never parsed for this.
pub fn has_unknown_name(toks: &[Token]) -> bool {
    toks.iter().enumerate().any(|(i, t)| match &t.tok {
        Tok::Ident(name) => !defined_by_jq(name, call_arity(&toks[i + 1..])),
        Tok::Str(parts) => parts
            .iter()
            .any(|p| matches!(p, StrPart::Interp(inner, _) if has_unknown_name(inner))),
        _ => false,
    })
}

/// Whether the tokens use a variable or label other than those every
/// program has: the cheap test before the scopes are checked.
pub fn has_variable(toks: &[Token], named: &[String]) -> bool {
    toks.iter().any(|t| match &t.tok {
        Tok::Binding(name) => !matches!(name.as_str(), "ENV" | "ARGS") && !named.contains(name),
        Tok::Str(parts) => parts
            .iter()
            .any(|p| matches!(p, StrPart::Interp(inner, _) if has_variable(inner, named))),
        _ => false,
    })
}

/// The arguments of a call whose name precedes `after`: none without a `(`,
/// else one more than its `;`s at the parenthesis' own depth.
fn call_arity(after: &[Token]) -> usize {
    if !matches!(after.first().map(|t| &t.tok), Some(Tok::Op("("))) {
        return 0;
    }
    let mut depth = 0usize;
    let mut arity = 1;
    for t in after {
        match &t.tok {
            Tok::Op("(" | "[" | "{") => depth += 1,
            Tok::Op(")" | "]" | "}") => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            Tok::Op(";") if depth == 1 => arity += 1,
            _ => {}
        }
    }
    arity
}

/// A name bound where a call is resolved: a definition, or a parameter.
struct Binding<'a> {
    name: &'a str,
    arity: usize,
    /// The definition, and the scope it was written in; `None` for a
    /// parameter.
    def: Option<&'a FuncDef>,
    /// The variables and labels in scope where the definition was written.
    vars: Vars<'a>,
    outer: Scope<'a>,
}

type Scope<'a> = Option<Rc<Binding<'a>>>;

fn bind<'a>(
    outer: &Scope<'a>,
    name: &'a str,
    arity: usize,
    def: Option<&'a FuncDef>,
    vars: &Vars<'a>,
) -> Rc<Binding<'a>> {
    Rc::new(Binding {
        name,
        arity,
        def,
        vars: vars.clone(),
        outer: outer.clone(),
    })
}

/// A `$name` or a `label $name` in scope.
struct Var<'a> {
    name: &'a str,
    label: bool,
    outer: Vars<'a>,
}

type Vars<'a> = Option<Rc<Var<'a>>>;

fn bind_var<'a>(outer: &Vars<'a>, name: &'a str, label: bool) -> Rc<Var<'a>> {
    Rc::new(Var {
        name,
        label,
        outer: outer.clone(),
    })
}

fn has_var(vars: &Vars<'_>, name: &str, label: bool) -> bool {
    let mut at = vars.as_ref();
    while let Some(v) = at {
        if v.name == name && v.label == label {
            return true;
        }
        at = v.outer.as_ref();
    }
    false
}

/// The variables every program has: `$ENV`, `$ARGS`, and the names the
/// command line binds (`--arg`, `--argjson`, `--slurpfile`, `--rawfile`).
/// `$__loc__` is no variable to the parser.
fn globals(named: &[String]) -> Vars<'_> {
    let mut vars = Some(bind_var(&None, "ENV", false));
    vars = Some(bind_var(&vars, "ARGS", false));
    for n in named {
        vars = Some(bind_var(&vars, n, false));
    }
    vars
}

/// The variables a pattern binds, every alternative's included.
fn pattern_vars<'a>(p: &'a Pattern, out: &mut Vec<&'a str>) {
    match p {
        Pattern::Var(v) => out.push(v),
        Pattern::Array(ps) => ps.iter().for_each(|p| pattern_vars(p, out)),
        Pattern::Object(ps) => {
            for p in ps {
                match p {
                    ObjPat::Var(v) => out.push(v),
                    ObjPat::VarPattern(v, p) => {
                        out.push(v);
                        pattern_vars(p, out);
                    }
                    ObjPat::Key(_, p) => pattern_vars(p, out),
                }
            }
        }
    }
}

/// What the walk is looking for: the reachable errors jq reports, or every
/// use of an undefined name, reachable or not.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Reachable,
    Everywhere,
}

struct Walk<'o> {
    mode: Mode,
    seen: Vec<*const FuncDef>,
    errors: &'o mut Vec<CompileError>,
    /// For [`Mode::Everywhere`]: the spans to replace, and their replacements.
    spans: &'o mut Vec<(Span, &'static str)>,
}

impl<'a> Walk<'_> {
    fn undefined_var(&mut self, name: &str, span: Span, label: bool) {
        match self.mode {
            Mode::Reachable => self.errors.push(CompileError {
                message: if label {
                    format!("$*label-{name} is not defined")
                } else {
                    format!("${name} is not defined")
                },
                span,
                more: Vec::new(),
            }),
            Mode::Everywhere => self.spans.push((span, "(empty)")),
        }
    }

    fn walk(&mut self, n: &'a Node, scope: &Scope<'a>, vars: &Vars<'a>) {
        match &n.kind {
            Kind::Def { def, rest } => {
                let inner = Some(bind(scope, &def.name, def.params.len(), Some(def), vars));
                if self.mode == Mode::Everywhere {
                    self.body(def, &inner, vars);
                }
                self.walk(rest, &inner, vars);
            }
            Kind::Call { name, args } => self.call(n, name, args, scope, vars),
            Kind::Var(name) => {
                if !has_var(vars, name, false) {
                    self.undefined_var(name, n.span, false);
                }
            }
            Kind::Break(name) => {
                if !has_var(vars, name, true) {
                    self.undefined_var(name, n.span, true);
                }
            }
            Kind::Label { name, body } => self.walk(body, scope, &Some(bind_var(vars, name, true))),
            Kind::Bind {
                source,
                patterns,
                body,
            } => {
                self.walk(source, scope, vars);
                let inner = self.patterns(patterns, scope, vars);
                self.walk(body, scope, &inner);
            }
            Kind::Reduce {
                source,
                patterns,
                init,
                update,
            } => self.fold(source, patterns, init, &[update], scope, vars),
            Kind::Foreach {
                source,
                patterns,
                init,
                update,
                extract,
            } => {
                let mut bodies = Vec::from([&**update]);
                bodies.extend(extract.as_deref());
                self.fold(source, patterns, init, &bodies, scope, vars);
            }
            Kind::Object(pairs) => {
                for p in pairs {
                    match p {
                        // jq locates the error over the whole entry.
                        ObjPair::KeyValue {
                            key: ObjKey::Var(v, at),
                            value,
                        } if !has_var(vars, v, false) => match self.mode {
                            Mode::Reachable => {
                                self.undefined_var(v, Span::new(at.start, value.span.end), false);
                            }
                            Mode::Everywhere => self.spans.push((*at, "(empty)")),
                        },
                        // `{(empty)}` is not jq's syntax: the shorthand
                        // becomes a whole entry.
                        ObjPair::Shorthand(ObjShort::Var(v, at)) if !has_var(vars, v, false) => {
                            match self.mode {
                                Mode::Reachable => self.undefined_var(v, *at, false),
                                Mode::Everywhere => self.spans.push((*at, "(empty): (empty)")),
                            }
                        }
                        _ => {}
                    }
                }
                for c in n.children() {
                    self.walk(c, scope, vars);
                }
            }
            _ => {
                for c in n.children() {
                    self.walk(c, scope, vars);
                }
            }
        }
    }

    /// A call: resolved in `scope`, its definition's body walked at the
    /// first call; an unknown name is jq's error, or a span to replace.
    fn call(
        &mut self,
        n: &'a Node,
        name: &'a str,
        args: &'a [Node],
        scope: &Scope<'a>,
        vars: &Vars<'a>,
    ) {
        let mut at = scope.as_ref();
        while let Some(b) = at {
            if b.name == name && b.arity == args.len() {
                break;
            }
            at = b.outer.as_ref();
        }
        // The whole call is replaced, its arguments with it.
        if self.mode == Mode::Everywhere && at.is_none() && !defined_by_jq(name, args.len()) {
            self.spans.push((n.span, "(empty)"));
            return;
        }
        for a in args {
            self.walk(a, scope, vars);
        }
        match at {
            Some(b) => {
                if self.mode == Mode::Reachable
                    && let Some(def) = b.def
                    && !self.seen.contains(&std::ptr::from_ref(def))
                {
                    self.seen.push(std::ptr::from_ref(def));
                    // The definition sees itself, for recursion.
                    let inner = Some(bind(
                        &b.outer,
                        &def.name,
                        def.params.len(),
                        Some(def),
                        &b.vars,
                    ));
                    self.body(def, &inner, &b.vars);
                }
            }
            None if !defined_by_jq(name, args.len()) => self.errors.push(CompileError {
                message: format!("{name}/{} is not defined", args.len()),
                span: Span::new(n.span.start, n.span.start + name.len()),
                more: Vec::new(),
            }),
            None => {}
        }
    }

    /// `reduce`/`foreach`: the source and the initial value see the outer
    /// scope, the update (and `foreach`'s extract) the pattern's variables.
    fn fold(
        &mut self,
        source: &'a Node,
        patterns: &'a [Pattern],
        init: &'a Node,
        bodies: &[&'a Node],
        scope: &Scope<'a>,
        vars: &Vars<'a>,
    ) {
        self.walk(source, scope, vars);
        let inner = self.patterns(patterns, scope, vars);
        self.walk(init, scope, vars);
        for b in bodies {
            self.walk(b, scope, &inner);
        }
    }

    /// A definition's body: its parameters bound, `$name` ones as
    /// variables too.
    fn body(&mut self, def: &'a FuncDef, scope: &Scope<'a>, vars: &Vars<'a>) {
        let (mut scope, mut vars) = (scope.clone(), vars.clone());
        for p in &def.params {
            let (Param::Filter(p) | Param::Value(p)) = p;
            scope = Some(bind(&scope, p.trim_start_matches('$'), 0, None, &None));
        }
        for p in &def.params {
            if let Param::Value(v) = p {
                vars = Some(bind_var(&vars, v.trim_start_matches('$'), false));
            }
        }
        self.walk(&def.body, &scope, &vars);
    }

    /// Walk the patterns' computed keys, in the outer scope, and return the
    /// scope with every variable they bind.
    fn patterns(
        &mut self,
        patterns: &'a [Pattern],
        scope: &Scope<'a>,
        vars: &Vars<'a>,
    ) -> Vars<'a> {
        let mut inner = vars.clone();
        for p in patterns {
            let mut keys = Vec::new();
            p.collect(&mut keys);
            for k in keys {
                self.walk(k, scope, vars);
            }
            let mut bound = Vec::new();
            pattern_vars(p, &mut bound);
            for v in bound {
                inner = Some(bind_var(&inner, v, false));
            }
        }
        inner
    }
}

/// The calls and variables jq rejects as undefined, in the order jq reports
/// them; spans are relative to the text `tree` was parsed from.
/// `named` are the variables the command line binds.
///
/// jq checks only reachable code: it drops a definition nothing calls before
/// it looks for undefined names, so `def f: isarray; 1` runs and
/// `def f: isarray; f` does not. A call's arguments are always checked, even
/// those the callee never uses. Each definition is walked once, when first
/// called, in the scope it was written in, with its parameters bound (a
/// `$name` parameter binds `name/0` too, as in jq). A variable is `$x is not
/// defined`, a label `$*label-x is not defined`.
pub fn undefined_calls(tree: &Node, named: &[String]) -> Vec<CompileError> {
    // Which definitions jq keeps: those something reachable calls.
    let (mut ignored, mut spans) = (Vec::new(), Vec::new());
    let mut walk = Walk {
        mode: Mode::Reachable,
        seen: Vec::new(),
        errors: &mut ignored,
        spans: &mut spans,
    };
    let vars = globals(named);
    walk.walk(tree, &None, &vars);
    let mut levels = Levels {
        reachable: walk.seen,
        errors: Vec::new(),
    };
    levels.function(tree, &None, &vars);
    levels.errors
}

/// A function body jq compiles on its own: a definition's, or a closure's
/// (an argument to anything but a C builtin, either side of `=`/`|=`, the
/// target of `op=`).
struct Pending<'a> {
    body: &'a Node,
    scope: Scope<'a>,
    vars: Vars<'a>,
}

/// The order jq's `compile` reports undefined names in. Within one
/// function body, the names are found in the order of its instructions,
/// which is the source's except where jq's code generation reorders it (an
/// index before its base, a `reduce`'s initial value before its source),
/// and a C builtin's arguments are compiled inline. The function's
/// closures and definitions are compiled after it, in order, and only if it
/// had no error itself.
struct Levels {
    reachable: Vec<*const FuncDef>,
    errors: Vec<CompileError>,
}

impl<'a> Levels {
    fn function(&mut self, body: &'a Node, scope: &Scope<'a>, vars: &Vars<'a>) {
        let before = self.errors.len();
        let mut pending = Vec::new();
        self.level(body, scope, vars, &mut pending);
        if self.errors.len() == before {
            for p in pending {
                self.function(p.body, &p.scope, &p.vars);
            }
        }
    }

    fn undefined_var(&mut self, message: String, span: Span) {
        self.errors.push(CompileError {
            message,
            span,
            more: Vec::new(),
        });
    }

    /// A `$name` used by an object entry, which has no node of its own.
    fn var_at(&mut self, name: &str, at: Span, vars: &Vars<'a>) {
        if !has_var(vars, name, false) {
            self.undefined_var(format!("${name} is not defined"), at);
        }
    }

    fn closure(body: &'a Node, scope: &Scope<'a>, vars: &Vars<'a>, pending: &mut Vec<Pending<'a>>) {
        pending.push(Pending {
            body,
            scope: scope.clone(),
            vars: vars.clone(),
        });
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one arm per construct jq orders its own way; the match is the index of them"
    )]
    fn level(
        &mut self,
        n: &'a Node,
        scope: &Scope<'a>,
        vars: &Vars<'a>,
        pending: &mut Vec<Pending<'a>>,
    ) {
        match &n.kind {
            Kind::Def { def, rest } => {
                let inner = Some(bind(scope, &def.name, def.params.len(), Some(def), vars));
                if self.reachable.contains(&std::ptr::from_ref(&**def)) {
                    let (mut body_scope, mut body_vars) = (inner.clone(), vars.clone());
                    for p in &def.params {
                        let (Param::Filter(p) | Param::Value(p)) = p;
                        body_scope =
                            Some(bind(&body_scope, p.trim_start_matches('$'), 0, None, &None));
                    }
                    for p in &def.params {
                        if let Param::Value(v) = p {
                            body_vars =
                                Some(bind_var(&body_vars, v.trim_start_matches('$'), false));
                        }
                    }
                    pending.push(Pending {
                        body: &def.body,
                        scope: body_scope,
                        vars: body_vars,
                    });
                }
                self.level(rest, &inner, vars, pending);
            }
            Kind::Call { name, args } => {
                let mut at = scope.as_ref();
                while let Some(b) = at {
                    if b.name == name && b.arity == args.len() {
                        break;
                    }
                    at = b.outer.as_ref();
                }
                match at {
                    None if !defined_by_jq(name, args.len()) => {
                        // jq does not look into an unknown call's arguments.
                        self.undefined_var(
                            format!("{name}/{} is not defined", args.len()),
                            Span::new(n.span.start, n.span.start + name.len()),
                        );
                    }
                    None if super::jq_tables::C_BUILTINS
                        .binary_search(&(name.as_str(), args.len()))
                        .is_ok() =>
                    {
                        for a in args {
                            self.level(a, scope, vars, pending);
                        }
                    }
                    // A definition, a parameter, or a builtin jq defines in
                    // jq: the arguments are closures.
                    _ => {
                        for a in args {
                            Self::closure(a, scope, vars, pending);
                        }
                    }
                }
            }
            Kind::Var(name) => {
                if !has_var(vars, name, false) {
                    self.undefined_var(format!("${name} is not defined"), n.span);
                }
            }
            Kind::Break(name) => {
                if !has_var(vars, name, true) {
                    self.undefined_var(format!("$*label-{name} is not defined"), n.span);
                }
            }
            Kind::Label { name, body } => {
                self.level(body, scope, &Some(bind_var(vars, name, true)), pending);
            }
            Kind::Bind {
                source,
                patterns,
                body,
            } => {
                self.level(source, scope, vars, pending);
                let inner = self.patterns(patterns, scope, vars, pending);
                self.level(body, scope, &inner, pending);
            }
            // jq's `gen_reduce`/`gen_foreach` evaluate the initial value first.
            Kind::Reduce {
                source,
                patterns,
                init,
                update,
            } => {
                self.level(init, scope, vars, pending);
                self.level(source, scope, vars, pending);
                let inner = self.patterns(patterns, scope, vars, pending);
                self.level(update, scope, &inner, pending);
            }
            Kind::Foreach {
                source,
                patterns,
                init,
                update,
                extract,
            } => {
                self.level(init, scope, vars, pending);
                self.level(source, scope, vars, pending);
                let inner = self.patterns(patterns, scope, vars, pending);
                self.level(update, scope, &inner, pending);
                if let Some(e) = extract {
                    self.level(e, scope, &inner, pending);
                }
            }
            // `L = R` is `_assign(L; R)` and `L |= R` is `_modify(L; R)`:
            // both sides are closures. `L op= R` is
            // `R as $t | _modify(L; . op $t)`: only `L` is one.
            Kind::Binary { op, lhs, rhs } if matches!(*op, "=" | "|=") => {
                Self::closure(lhs, scope, vars, pending);
                Self::closure(rhs, scope, vars, pending);
            }
            Kind::Binary { op, lhs, rhs }
                if matches!(*op, "+=" | "-=" | "*=" | "/=" | "%=" | "//=") =>
            {
                self.level(rhs, scope, vars, pending);
                Self::closure(lhs, scope, vars, pending);
            }
            // `gen_index`: the key, then the base.
            Kind::Index { base, index } => {
                self.level(index, scope, vars, pending);
                self.level(base, scope, vars, pending);
            }
            Kind::Slice { base, from, to } => {
                for b in [from, to].into_iter().flatten() {
                    self.level(b, scope, vars, pending);
                }
                self.level(base, scope, vars, pending);
            }
            Kind::Field { base, name } => {
                if let FieldName::Str(s) = name {
                    let mut keys = Vec::new();
                    s.collect(&mut keys);
                    for k in keys {
                        self.level(k, scope, vars, pending);
                    }
                }
                self.level(base, scope, vars, pending);
            }
            Kind::Object(pairs) => {
                for p in pairs {
                    match p {
                        ObjPair::KeyValue { key, value } => {
                            match key {
                                ObjKey::Var(v, at) => {
                                    self.var_at(v, Span::new(at.start, value.span.end), vars);
                                }
                                ObjKey::Expr(e) => self.level(e, scope, vars, pending),
                                ObjKey::Str(s) => {
                                    let mut keys = Vec::new();
                                    s.collect(&mut keys);
                                    for k in keys {
                                        self.level(k, scope, vars, pending);
                                    }
                                }
                                ObjKey::Ident(_) => {}
                            }
                            self.level(value, scope, vars, pending);
                        }
                        ObjPair::Shorthand(ObjShort::Var(v, at)) => self.var_at(v, *at, vars),
                        ObjPair::Shorthand(ObjShort::Str(s)) => {
                            let mut keys = Vec::new();
                            s.collect(&mut keys);
                            for k in keys {
                                self.level(k, scope, vars, pending);
                            }
                        }
                        ObjPair::Shorthand(_) => {}
                    }
                }
            }
            _ => {
                for c in n.children() {
                    self.level(c, scope, vars, pending);
                }
            }
        }
    }

    fn patterns(
        &mut self,
        patterns: &'a [Pattern],
        scope: &Scope<'a>,
        vars: &Vars<'a>,
        pending: &mut Vec<Pending<'a>>,
    ) -> Vars<'a> {
        let mut inner = vars.clone();
        for p in patterns {
            let mut keys = Vec::new();
            p.collect(&mut keys);
            for k in keys {
                self.level(k, scope, vars, pending);
            }
            let mut bound = Vec::new();
            pattern_vars(p, &mut bound);
            for v in bound {
                inner = Some(bind_var(&inner, v, false));
            }
        }
        inner
    }
}

/// Uses of names jq does not define anywhere, reachable or not: whole calls,
/// and variables and `break`s, each with its replacement; spans are
/// relative to the text `tree` was parsed from.
///
/// jq never compiles a definition nothing calls, so in such a definition an
/// undefined name is harmless, where the engine compiles every definition
/// and refuses it. The caller replaces these uses by `empty` once
/// [`undefined_calls`] has found none reachable.
pub fn unreachable_undefined(tree: &Node, named: &[String]) -> Vec<(Span, &'static str)> {
    let (mut errors, mut spans) = (Vec::new(), Vec::new());
    let mut walk = Walk {
        mode: Mode::Everywhere,
        seen: Vec::new(),
        errors: &mut errors,
        spans: &mut spans,
    };
    walk.walk(tree, &None, &globals(named));
    spans.sort_by_key(|(s, _)| s.start);
    spans.dedup_by_key(|(s, _)| *s);
    spans
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
        more: Vec::new(),
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
    use super::{module_header, program, undefined_calls};
    use crate::syntax::parse::parse;

    fn undefined(src: &str) -> Vec<String> {
        let tree = parse(src).expect("parses");
        undefined_calls(&tree, &["named".to_owned()])
            .into_iter()
            .map(|e| format!("{}@{}", e.message, e.span.start))
            .collect()
    }

    /// Measured against jq 1.8.1, which drops unreferenced definitions
    /// before it looks for undefined names.
    #[test]
    fn engine_only_calls_are_undefined_in_reachable_code_only() {
        assert_eq!(undefined("isarray"), ["isarray/0 is not defined@0"]);
        assert_eq!(
            undefined("isarray, isstring"),
            ["isarray/0 is not defined@0", "isstring/0 is not defined@9"]
        );
        assert_eq!(undefined("[.[] | halt(1)]"), ["halt/1 is not defined@7"]);
        assert!(undefined("halt").is_empty(), "jq has halt/0");
        assert_eq!(undefined("def f: isnumber; 1"), [] as [String; 0]);
        assert_eq!(
            undefined("def f: isnumber; f"),
            ["isnumber/0 is not defined@7"]
        );
        assert_eq!(undefined("def isarray: 1; isarray"), [] as [String; 0]);
        assert_eq!(
            undefined("def f(g): 1; f(isarray)"),
            ["isarray/0 is not defined@15"],
            "arguments are checked even when unused"
        );
        assert_eq!(undefined("def f: isarray; def f: 1; f"), [] as [String; 0]);
        assert_eq!(
            undefined("def f: 1; def g: f; def f: isarray; g"),
            [] as [String; 0]
        );
        assert_eq!(
            undefined("def f(x): def x: isarray; 1; f(.)"),
            [] as [String; 0]
        );
        assert_eq!(
            undefined("def f($isarray): isarray; f(1)"),
            [] as [String; 0]
        );
        assert_eq!(
            undefined("def r: if . > 0 then .-1|r else isarray end; 1|r"),
            ["isarray/0 is not defined@32"]
        );
    }

    #[test]
    fn undefined_calls_in_unreachable_code_are_found_lexically() {
        let spans = |src: &str| {
            let tree = parse(src).expect("parses");
            super::unreachable_undefined(&tree, &[])
                .into_iter()
                .map(|(s, _)| &src[s.start..s.end])
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        assert_eq!(spans("def f: isnumber; 1"), ["isnumber"]);
        assert_eq!(spans("def g: tobytes(1); [1]"), ["tobytes(1)"]);
        assert_eq!(spans("def f(x): x; f(1)"), [] as [String; 0]);
        assert_eq!(spans("def isarray: 1; isarray"), [] as [String; 0]);
        assert_eq!(
            spans("def f: $x + 1; def g: break $l; 1"),
            ["$x", "break $l"]
        );
        assert_eq!(spans("def f: nosuch($x); 1"), ["nosuch($x)"]);
        // An object entry's own `$name`, never another's or a string's.
        assert_eq!(spans("def f: {$xx: 1, $x: 2}; 1"), ["$xx", "$x"]);
        assert_eq!(spans("def f: {a: \"$x\", $x}; 1"), ["$x"]);
    }

    /// `{(empty)}` is not jq's syntax: a shorthand becomes a whole entry.
    #[test]
    fn an_undefined_shorthand_is_replaced_by_an_entry() {
        let src = "def f: {a: \"$x\", $x}; 1";
        let tree = parse(src).expect("parses");
        let found = super::unreachable_undefined(&tree, &[]);
        let [(span, with)] = found.as_slice() else {
            panic!("{found:?}");
        };
        assert_eq!((span.start, *with), (17, "(empty): (empty)"));
    }

    /// Measured against jq 1.8.1: the scopes of variables and labels.
    #[test]
    fn undefined_variables_and_labels_follow_jqs_scopes() {
        assert_eq!(undefined("$x"), ["$x is not defined@0"]);
        assert_eq!(undefined("[1, $foo]"), ["$foo is not defined@4"]);
        assert_eq!(
            undefined("$ENV, $ARGS, $named, $__loc__"),
            [] as [String; 0]
        );
        assert_eq!(undefined("$__prog_args"), ["$__prog_args is not defined@0"]);
        assert_eq!(
            undefined("label $a | break $b"),
            ["$*label-b is not defined@11"]
        );
        assert_eq!(
            undefined("label $f | def g: break $f; g"),
            [] as [String; 0]
        );
        assert_eq!(undefined("def f: $x; 1"), [] as [String; 0]);
        assert_eq!(
            undefined("def f: def g: $q; g; f"),
            ["$q is not defined@14"]
        );
        assert_eq!(undefined("def f($x): $x; $x"), ["$x is not defined@15"]);
        assert_eq!(undefined("(. as $a | $a), $a"), ["$a is not defined@16"]);
        assert_eq!(undefined(". as $x | def f: $x; f"), [] as [String; 0]);
        assert_eq!(
            undefined("reduce 1 as $x ($x; .)"),
            ["$x is not defined@16"]
        );
        assert_eq!(undefined("foreach 1 as $x (0; .; $x)"), [] as [String; 0]);
        assert_eq!(
            undefined("reduce . as $r (0; $r) | $r"),
            ["$r is not defined@25"]
        );
        assert_eq!(
            undefined(". as {a: $x, ($x): $y} | $y"),
            ["$x is not defined@14"]
        );
        assert_eq!(
            undefined(". as [$a] ?// {b: $c} | $a, $c"),
            [] as [String; 0]
        );
        assert_eq!(undefined(". as {$a: [$b]} | [$a, $b]"), [] as [String; 0]);
        assert_eq!(undefined("{$zz, a: 1}"), ["$zz is not defined@1"]);
        assert_eq!(undefined("\"\\($zz)\""), ["$zz is not defined@3"]);
        assert_eq!(
            undefined("$x, $y"),
            ["$x is not defined@0", "$y is not defined@4"]
        );
    }

    #[test]
    fn several_errors_are_counted_as_jq_counts_them() {
        let tree = parse("isarray, isstring").expect("parses");
        let mut all = undefined_calls(&tree, &[]).into_iter();
        let mut e = all.next().expect("one");
        e.more = all.collect();
        let text = e.render("jq", "isarray, isstring");
        assert!(text.ends_with("jq: 2 compile errors\n"), "{text}");
        assert_eq!(text.matches("is not defined").count(), 2);
    }

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
