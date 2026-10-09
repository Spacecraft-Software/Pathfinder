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

/// Environment escape hatch: set it to hand every program to jaq unparsed.
pub const NO_REWRITE_ENV: &str = "PATHFINDER_NO_REWRITE";

/// Rewrite `src` if it needs it; `None` means "pass it through unchanged".
///
/// Cheap when nothing applies: a token pre-screen decides whether to parse at
/// all, and most programs contain no assignment and no compound `reduce`
/// source. A program the parser rejects is also passed through — exactly what
/// happened before the parser existed.
pub fn rewrite(src: &str) -> Option<Rewrite> {
    if std::env::var_os(NO_REWRITE_ENV).is_some() || !worth_parsing(src) {
        return None;
    }
    let tree = parse(src).ok()?;
    let mut cx = Cx {
        src,
        needs: Vec::new(),
        temps: 0,
        fast_del: !defines_any(&tree, FAST_DEL_RELIES_ON),
        keys: Vec::new(),
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
    fn any(toks: &[super::lex::Token]) -> bool {
        toks.iter().any(|t| match &t.tok {
            // Assignment; `reduce`/`foreach` sources; `?//` alternatives and
            // `{$b: pattern}` (a binding before `:`).
            Tok::Op("=" | "|=" | "+=" | "-=" | "*=" | "/=" | "%=" | "//=" | "?//")
            | Tok::Keyword(Kw::Reduce | Kw::Foreach)
            | Tok::Binding(_) => true,
            Tok::Ident(name) => name == "del",
            Tok::Format(name) => matches!(name.as_str(), "base64d" | "urid"),
            Tok::Str(parts) => parts
                .iter()
                .any(|p| matches!(p, StrPart::Interp(inner, _) if any(inner))),
            _ => false,
        })
    }
    lex(src).is_ok_and(|toks| any(&toks) || super::check::has_computed_key(&toks))
}

struct Cx<'a> {
    src: &'a str,
    needs: Vec<&'static str>,
    /// Counter for `$__pf_tN` temporaries, unique per program.
    temps: usize,
    /// Whether `del(f)` may be routed to jaq's native `del`: false when the
    /// program defines a name [`single_path`] reasons about.
    fast_del: bool,
    /// Computed object keys (`{(k): v}`, the parenthesised node) awaiting the
    /// string check jaq leaves out.
    keys: Vec<super::Span>,
}

impl Cx<'_> {
    fn need(&mut self, name: &'static str) {
        if !self.needs.contains(&name) {
            self.needs.push(name);
        }
    }

    /// Emit `node`; the flag says whether anything in it was rewritten.
    fn emit(&mut self, node: &Node) -> (String, bool) {
        match &node.kind {
            // jaq builds `{1: 2}` — not JSON — from a computed key that is not
            // a string, where jq raises an error. Check the key first.
            Kind::Paren(inner) if self.keys.contains(&node.span) => {
                let (k, _) = self.emit(inner);
                self.need("_pf_dump");
                (format!("(({k}) | {KEY_CHECK})"), true)
            }
            Kind::Object(pairs) => {
                let before = self.keys.len();
                for p in pairs {
                    if let super::ObjPair::KeyValue {
                        key: super::ObjKey::Expr(k),
                        ..
                    } = p
                        && !matches!(&k.kind, Kind::Paren(q) if matches!(q.kind, Kind::Str(_)))
                    {
                        self.keys.push(k.span);
                    }
                }
                let out = self.splice(node);
                self.keys.truncate(before);
                out
            }
            // `L |= empty` is `del(L)` in jq 1.7+: `_modify` collects the paths
            // `empty` produced nothing for and deletes them together.
            Kind::Binary { op, lhs, rhs }
                if self.fast_del
                    && *op == "|="
                    && matches!(&rhs.kind, Kind::Call { name, args } if name == "empty" && args.is_empty()) =>
            {
                let text = self.delete(lhs).unwrap_or_else(|| {
                    self.need("_pf_delpaths");
                    let (l, _) = self.emit(lhs);
                    format!("_pf_delpaths([path({l})])")
                });
                (format!("({text})"), true)
            }
            Kind::Binary { op, lhs, rhs } if is_assignment(op) => {
                let (l, _) = self.emit(lhs);
                let (r, _) = self.emit(rhs);
                (self.assignment(op, lhs, &l, &r), true)
            }
            Kind::Bind {
                source,
                patterns,
                body,
            } if patterns.len() > 1 || patterns.iter().any(binds_and_destructures) => {
                (self.bind(source, patterns, body), true)
            }
            Kind::Reduce {
                source,
                patterns,
                init,
                update,
            } if patterns.len() == 1 && binds_and_destructures(&patterns[0]) => {
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
            } if patterns.len() == 1 && binds_and_destructures(&patterns[0]) => {
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
            Kind::Reduce { source, .. } | Kind::Foreach { source, .. } if is_compound(source) => {
                (self.splice_wrapping(node, Some(source.span)).0, true)
            }
            Kind::Call { name, args } if self.fast_del && name == "del" && args.len() == 1 => {
                match self.delete(&args[0]) {
                    Some(text) => (text, true),
                    None => self.splice(node),
                }
            }
            // jaq's decoders are stricter than jq's in some places and more
            // lenient in others; the prelude has jq's rules.
            Kind::Format(f) if f == "base64d" || f == "urid" => {
                let name = if f == "base64d" {
                    "_pf_base64d"
                } else {
                    "_pf_urid"
                };
                self.need(name);
                (format!("({name})"), true)
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
        let span = node.span;
        let mut out = String::with_capacity(span.end - span.start);
        let mut cursor = span.start;
        let mut dirty = false;
        for child in node.children() {
            // Zero-width synthetic children (the `.` implied by `.foo`) have no
            // text of their own and can never be rewritten.
            if child.span.start == child.span.end {
                continue;
            }
            let (text, child_dirty) = self.emit(child);
            out.push_str(&self.src[cursor..child.span.start]);
            if wrap == Some(child.span) {
                out.push('(');
                out.push_str(&text);
                out.push(')');
                dirty = true;
            } else {
                out.push_str(&text);
            }
            cursor = child.span.end;
            dirty |= child_dirty;
        }
        out.push_str(&self.src[cursor..span.end]);
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
        self.temps += 1;
        // jaq reads `.[0]` of an object as `null` where jq raises an error, so
        // an array pattern would match an object and the next alternative would
        // never be tried. Check first, with jq's message.
        let alt = |p: &str, pat: &super::Pattern| {
            let check =
                array_check(pat).map_or_else(String::new, |c| format!("({v} | {c} | empty), "));
            format!("({nulls}{check}({v} as {p} | {b}))")
        };
        let mut alts = pats.iter().zip(patterns).rev();
        let (last, last_pat) = alts.next().expect("more than one pattern");
        let mut text = alt(last, last_pat);
        for (p, pat) in alts {
            text = format!("(try {} catch {text})", alt(p, pat));
        }
        format!("({s} as {v} | {text})")
    }

    /// `del(target)` through a fast route, when the target allows one.
    ///
    /// jaq's own `del` is linear, but agrees with jq only for a target that
    /// yields one path per array (see [`single_path`]) and whose deleting step
    /// is not a named key, which jaq removes by swapping the last key into its
    /// place. Named keys are removed by rebuilding the object in order instead.
    /// Either way, where jq succeeds and jaq errors (a `null` container, an
    /// index out of range) or yields nothing (deleting the root), the exact
    /// algorithm runs on the original input and gives jq's answer or error.
    fn delete(&mut self, target: &Node) -> Option<String> {
        if !single_path(target) {
            return None;
        }
        // How the last step deletes: by name, by position, or by iterating.
        let step = deleting_step(target);
        let (prefix, leaf) = match &step.kind {
            Kind::Field { .. } | Kind::Comma(..) => {
                let (prefix, keys) = field_split(target, self.src)?;
                (prefix, Leaf::Keys(keys))
            }
            Kind::Index { index, .. } if matches!(index.kind, Kind::Str(_)) => {
                let (prefix, keys) = field_split(target, self.src)?;
                (prefix, Leaf::Keys(keys))
            }
            Kind::Index { .. } | Kind::Slice { .. } => {
                let (prefix, at) = position_split(target)?;
                (prefix, Leaf::At(at))
            }
            _ => (None, Leaf::Native),
        };
        if prefix.is_some_and(|p| !update_prefix(p)) {
            return None;
        }
        self.need("_pf_delpaths");
        let (t, _) = self.emit(target);
        let v = format!("$__pf_t{}", self.temps);
        self.temps += 1;
        let exact = format!("({v} | _pf_delpaths([path({t})]))");
        let leaf = match leaf {
            Leaf::Native => {
                self.need("_pf_del0");
                return Some(format!(
                    "(. as {v} | try first(_pf_del0({t}), {exact}) catch {exact})"
                ));
            }
            Leaf::Keys(keys) => {
                self.need("_pf_del0");
                delete_keys(&keys)
            }
            Leaf::At(at) => {
                self.need("_pf_delat");
                format!("_pf_delat({})", self.step_on_dot(at))
            }
        };
        let fast = match prefix {
            None => leaf,
            Some(p) => {
                let (p, _) = self.emit(p);
                format!("(({p}) |= {leaf})")
            }
        };
        Some(format!("(. as {v} | try {fast} catch {exact})"))
    }

    /// An index or slice step re-based on `.`: `.a[0]` gives `.[0]`.
    fn step_on_dot(&mut self, step: &Node) -> String {
        let mut part = |n: Option<&Node>| n.map_or_else(String::new, |n| self.emit(n).0);
        match &step.kind {
            Kind::Index { index, .. } => format!(".[{}]", part(Some(index))),
            Kind::Slice { from, to, .. } => {
                let (f, t) = (part(from.as_deref()), part(to.as_deref()));
                format!(".[{f}:{t}]")
            }
            _ => unreachable!("position_split returns only index and slice steps"),
        }
    }

    /// Emit an assignment with jq's semantics.
    ///
    /// The common case prepares the document with `_pf_vivify` and then lets
    /// jaq's own operator run, which is fast and — once the containers exist —
    /// agrees with jq. A target with an explicit comma (`.[1,2] |= empty`) is
    /// the one place jaq's native update still differs: it deletes several
    /// explicit indices in order, shifting the later ones. Those go through
    /// jq's own `_modify`, which is only slow when there are many paths, and an
    /// explicit comma never produces many.
    ///
    /// `=` evaluates its right-hand side before the pre-pass, on the original
    /// input, as jq does; the update forms evaluate it per path, on the old value.
    fn assignment(&mut self, op: &str, lhs: &Node, l: &str, r: &str) -> String {
        let comma = has_comma(lhs);
        let t = format!("$__pf_t{}", self.temps);
        self.temps += 1;
        if comma && op != "=" {
            self.need("_pf_modify");
            return if op == "|=" {
                format!("_pf_modify(({l}); ({r}))")
            } else {
                let bin = &op[..op.len() - 1];
                format!("(({r}) as {t} | _pf_modify(({l}); . {bin} {t}))")
            };
        }
        if !addresses(lhs) {
            // Nothing in the target can name a missing location, so jaq's own
            // operator is already jq's — no pre-pass, full native speed.
            return match op {
                "|=" => format!("(({l}) |= ({r}))"),
                _ => format!("(({r}) as {t} | ({l}) {op} {t})"),
            };
        }
        self.need("_pf_vivify");
        if let Some(keys) = literal_keys(lhs, self.src) {
            // A literal target on `.`: prepare it inline, then update natively.
            let prep = literal_guard(&keys);
            return match op {
                "|=" => format!("({prep} | ({l}) |= ({r}))"),
                _ => format!("(({r}) as {t} | {prep} | ({l}) {op} {t})"),
            };
        }
        if let Some((outer, keys)) = split_target(lhs, self.src) {
            // `(A | B) op …` as `A |= (prepare B | B op …)`.
            let prep = literal_guard(&keys);
            let inner = chain_text(&keys);
            return match op {
                "|=" => format!("(({outer}) |= ({prep} | {inner} |= ({r})))"),
                _ => format!("(({r}) as {t} | ({outer}) |= ({prep} | {inner} {op} {t}))"),
            };
        }
        match op {
            "|=" => format!("(_pf_vivify(({l})) | ({l}) |= ({r}))"),
            _ => format!("(({r}) as {t} | _pf_vivify(({l})) | ({l}) {op} {t})"),
        }
    }
}

/// One key of a literal target.
enum Key {
    /// A JSON string literal, quotes included.
    Str(String),
    Idx(u32),
}

/// The inline pre-pass for a literal target.
///
/// If the leaf's parent already exists with the right type and the index is in
/// range, nothing happens. Otherwise each container on the way is built
/// top-down with jaq's own `|=` — `{}` for a field name, `null`-padded arrays
/// for an index — and a container of the wrong type raises jq's message right
/// there, with no separate validation pass.
fn literal_guard(keys: &[Key]) -> String {
    let lit = |k: &Key| match k {
        Key::Str(s) => s.clone(),
        Key::Idx(i) => i.to_string(),
    };
    let path_expr = |ks: &[Key]| -> String {
        if ks.is_empty() {
            ".".to_owned()
        } else {
            // `.[a][b]`, not `.[a].[b]`: jaq 3.0 does not parse the latter.
            ks.iter().fold(String::from("."), |mut out, k| {
                let _ = write!(out, "[{}]", lit(k));
                out
            })
        }
    };
    let getpath = |ks: &[Key]| -> String {
        if ks.is_empty() {
            ".".to_owned()
        } else {
            format!(
                "getpath([{}])",
                ks.iter().map(lit).collect::<Vec<_>>().join(",")
            )
        }
    };
    let right_type = |k: &Key| match k {
        Key::Str(_) => "\"object\"",
        Key::Idx(_) => "\"array\"",
    };
    let n = keys.len();
    let all: Vec<String> = keys.iter().map(lit).collect();

    let fits = match &keys[n - 1] {
        Key::Str(_) => "type == \"object\"".to_owned(),
        Key::Idx(i) => format!("type == \"array\" and length > {i}"),
    };
    let parent = getpath(&keys[..n - 1]);

    // Create each container top-down with jaq's own update operator.
    let steps: Vec<String> = (0..n)
        .map(|j| {
            let empty = match &keys[j] {
                Key::Str(_) => "{}",
                Key::Idx(_) => "[]",
            };
            let pad = match &keys[j] {
                Key::Idx(i) => format!(
                    " | if length <= {i} then . + [range(length; {i} + 1) | null] else . end"
                ),
                Key::Str(_) => String::new(),
            };
            let right = right_type(&keys[j]);
            // jq's message for a container of the wrong type. jq evaluates the
            // path before setting anything, so it is `jv_get`'s wording, with
            // a string key spelled out raw.
            let err = match &keys[j] {
                Key::Str(k) => {
                    format!("error(\"Cannot index \\(type) with string \\\"\" + {k} + \"\\\"\")")
                }
                Key::Idx(_) => "error(\"Cannot index \\(type) with number\")".to_owned(),
            };
            let body = format!(
                "(if type == \"null\" then {empty} elif type == {right} then . else {err} end{pad})"
            );
            if j == 0 {
                body
            } else {
                format!("({} |= {body})", path_expr(&keys[..j]))
            }
        })
        .collect();

    let _ = &all;
    format!(
        "(if (try ({parent} | {fits}) catch false) then . else {} end)",
        steps.join(" | ")
    )
}

/// Split a target into a part that only visits existing values and a literal
/// suffix: `.[].v` and `.[] | select(.x) | .v` both become (`.[]…`, `["v"]`).
///
/// `(A | B) |= f` means `A |= (B |= f)` — each value A visits is updated
/// independently — so the suffix can be prepared and updated per element with
/// jaq's own operator, instead of enumerating every path one by one.
fn split_target(node: &Node, src: &str) -> Option<(String, Vec<Key>)> {
    match &node.kind {
        Kind::Pipe(a, b) if !addresses(a) => {
            let a_text = src[a.span.start..a.span.end].to_owned();
            if let Some(keys) = literal_keys(b, src) {
                return Some((a_text, keys));
            }
            let (rest, keys) = split_target(b, src)?;
            Some((format!("{a_text} | {rest}"), keys))
        }
        Kind::Field { .. } | Kind::Index { .. } => {
            // A postfix chain over a non-addressing base: `.[].a.b`.
            let mut keys = Vec::new();
            let mut n = node;
            while let Kind::Field { base, .. } | Kind::Index { base, .. } = &n.kind {
                keys.push(single_key(n, src)?);
                n = base;
            }
            if matches!(n.kind, Kind::Identity) || addresses(n) {
                return None;
            }
            keys.reverse();
            Some((src[n.span.start..n.span.end].to_owned(), keys))
        }
        _ => None,
    }
}

/// The key a single Field/Index step adds, if it is a literal.
fn single_key(n: &Node, src: &str) -> Option<Key> {
    match &n.kind {
        Kind::Field {
            name: super::FieldName::Ident(f),
            ..
        } => Some(Key::Str(format!("\"{f}\""))),
        Kind::Field {
            name: super::FieldName::Str(s),
            ..
        } if s.format.is_none() && s.parts.iter().all(|p| matches!(p, super::StrSeg::Text(_))) => {
            match (s.parts.first(), s.parts.last()) {
                (Some(super::StrSeg::Text(a)), Some(super::StrSeg::Text(b))) => {
                    Some(Key::Str(src[a.start - 1..=b.end].to_owned()))
                }
                _ => Some(Key::Str("\"\"".to_owned())),
            }
        }
        Kind::Index { index, .. } if matches!(index.kind, Kind::Number) => {
            // Above jq's array-index limit, leave it to the generic walk, which
            // raises jq's "Array index too large" instead of padding.
            src[index.span.start..index.span.end]
                .parse::<u32>()
                .ok()
                .filter(|i| *i <= 536_870_911)
                .map(Key::Idx)
        }
        _ => None,
    }
}

/// A literal chain on `.`: its keys, outermost first.
fn literal_keys(node: &Node, src: &str) -> Option<Vec<Key>> {
    let mut keys = Vec::new();
    let mut n = node;
    loop {
        match &n.kind {
            Kind::Identity => break,
            Kind::Paren(inner) => n = inner,
            Kind::Field { base, .. } | Kind::Index { base, .. } => {
                keys.push(single_key(n, src)?);
                n = base;
            }
            _ => return None,
        }
    }
    keys.reverse();
    (!keys.is_empty()).then_some(keys)
}

/// `.[k1][k2]…` for a key list — without a `.` between brackets, which
/// jaq 3.0 does not parse.
fn chain_text(keys: &[Key]) -> String {
    keys.iter().fold(String::from("."), |mut out, k| {
        let _ = match k {
            Key::Str(s) => write!(out, "[{s}]"),
            Key::Idx(i) => write!(out, "[{i}]"),
        };
        out
    })
}

/// Whether an assignment target can name a location that may not exist yet.
///
/// Conservative: only targets built entirely from steps that visit existing
/// values — `.`, `.[]`, `..`, `select(…)`, `recurse`, pipes, `?` — answer
/// `false`. Any field, index, slice, variable or other call answers `true`.
fn addresses(node: &Node) -> bool {
    match &node.kind {
        Kind::Identity | Kind::Recurse => false,
        Kind::Iterate(base) | Kind::Optional(base) | Kind::Paren(base) => addresses(base),
        Kind::Pipe(a, b) => addresses(a) || addresses(b),
        Kind::Call { name, args } => !matches!(
            (name.as_str(), args.len()),
            ("select", 1) | ("recurse" | "empty", 0)
        ),
        _ => true,
    }
}

/// jq's check on a computed object key, with its message: the value is shown
/// as `jv_dump_string_trunc` shows it, cut to 11 bytes plus `...`.
const KEY_CHECK: &str = "if . >= \"\" and . < [] then . \
     else error(\"Cannot use \\(type) (\\(_pf_dump)) as object key\") end";

/// Names whose builtin meaning [`single_path`] and [`single_valued`] assume.
const FAST_DEL_RELIES_ON: &[&str] = &[
    "del",
    "select",
    "recurse",
    "empty",
    "not",
    "length",
    "type",
    "has",
    "test",
    "startswith",
    "endswith",
    "contains",
    "inside",
    "ascii_downcase",
    "ascii_upcase",
    "tostring",
    "tonumber",
    "keys",
    "isnan",
    "in",
    "any",
    "all",
    "IN",
    "ltrimstr",
    "rtrimstr",
    "utf8bytelength",
    "tojson",
    "floor",
    "abs",
    "isempty",
];

/// Whether the program defines any of `names` itself.
fn defines_any(node: &Node, names: &[&str]) -> bool {
    if let Kind::Def { def, .. } = &node.kind
        && names.contains(&def.name.as_str())
    {
        return true;
    }
    node.children().into_iter().any(|c| defines_any(c, names))
}

/// Whether a `del` target yields at most one path into any one array.
///
/// That is the shape where jaq's native `del` deletes what jq's does: it walks
/// the target once, so a single path per container never shifts another. Two
/// explicit indices into one array (`.[0,1]`, `.[3], .[5]`) are the case it
/// gets silently wrong, deleting the second against the already-shortened
/// array. Commas are therefore allowed only between field names, which do not
/// shift, and `select`'s condition must produce at most one value, since each
/// `true` would emit the path again.
fn single_path(node: &Node) -> bool {
    match &node.kind {
        Kind::Identity | Kind::Recurse => true,
        Kind::Iterate(base) | Kind::Optional(base) | Kind::Paren(base) => single_path(base),
        Kind::Field { base, name } => single_path(base) && plain_field(name),
        Kind::Index { base, index } => single_path(base) && literal_index(index),
        Kind::Slice { base, from, to } => {
            single_path(base)
                && from.as_deref().is_none_or(literal_index)
                && to.as_deref().is_none_or(literal_index)
        }
        Kind::Pipe(a, b) => single_path(a) && single_path(b),
        Kind::Comma(..) => fields_only(node),
        Kind::Call { name, args } => match (name.as_str(), args.as_slice()) {
            ("select", [cond]) => single_valued(cond),
            ("recurse" | "empty", []) => true,
            _ => false,
        },
        _ => false,
    }
}

/// Inline, order-preserving deletion of named keys from an object.
///
/// jaq removes a key by swapping the last key into its place, so its own `del`
/// keeps jq's order only when the key is the last one; otherwise the object is
/// rebuilt without the keys. Inline rather than a prelude function, which
/// measured twice as slow per object. Anything but an object is refused, for
/// the caller's fallback to answer as jq does.
fn delete_keys(keys: &[String]) -> String {
    let present = keys
        .iter()
        .map(|k| format!("has({k})"))
        .collect::<Vec<_>>()
        .join(" or ");
    let kept = keys
        .iter()
        .map(|k| format!(". != {k}"))
        .collect::<Vec<_>>()
        .join(" and ");
    let rebuild = format!(
        ". as $__pf_o | reduce (keys_unsorted[] | select({kept})) as $__pf_k ({{}}; .[$__pf_k] = $__pf_o[$__pf_k])"
    );
    let body = match keys {
        [k] => format!(
            "if has({k}) | not then . elif (keys_unsorted | .[-1]) == {k} then _pf_del0(.[{k}]) else {rebuild} end"
        ),
        _ => format!("if ({present}) | not then . else {rebuild} end"),
    };
    format!("(if type == \"object\" then ({body}) else error(\"pathfinder: not an object\") end)")
}

/// What the deleting step of a `del` target removes.
enum Leaf<'a> {
    /// Named keys, as JSON string literals.
    Keys(Vec<String>),
    /// One index or slice; the node is the step itself.
    At(&'a Node),
    /// Whatever iteration reached — jaq's own `del` handles it.
    Native,
}

/// Split a target whose deleting step is an index or slice into the prefix
/// leading to the array (`None` for `.`) and that step.
fn position_split(n: &Node) -> Option<(Option<&Node>, &Node)> {
    match &n.kind {
        Kind::Paren(x) => position_split(x),
        Kind::Index { base, .. } | Kind::Slice { base, .. } => Some((prefix_of(base), n)),
        Kind::Pipe(p, step) if matches!(&step.kind, Kind::Index { base, .. } | Kind::Slice { base, .. } if matches!(base.kind, Kind::Identity)) => {
            Some((prefix_of(p), step))
        }
        _ => None,
    }
}

/// The step of a `del` target that does the deleting: the last one that moves
/// to another location, looking through filters such as `select`.
fn deleting_step(n: &Node) -> &Node {
    match &n.kind {
        Kind::Pipe(a, b) if only_filters(b) => deleting_step(a),
        Kind::Pipe(_, b) => deleting_step(b),
        Kind::Paren(x) | Kind::Optional(x) => deleting_step(x),
        _ => n,
    }
}

/// Steps that pass `.` through or drop it, never moving.
fn only_filters(n: &Node) -> bool {
    match &n.kind {
        Kind::Identity => true,
        Kind::Pipe(a, b) => only_filters(a) && only_filters(b),
        Kind::Paren(x) => only_filters(x),
        Kind::Call { name, args } => {
            matches!((name.as_str(), args.len()), ("select", 1) | ("empty", 0))
        }
        _ => false,
    }
}

/// Split a target that deletes named keys into the prefix leading to the
/// object (`None` for `.` itself) and the keys, as JSON string literals.
fn field_split<'a>(n: &'a Node, src: &str) -> Option<(Option<&'a Node>, Vec<String>)> {
    match &n.kind {
        Kind::Paren(x) => field_split(x, src),
        Kind::Field { base, .. } | Kind::Index { base, .. } => {
            Some((prefix_of(base), vec![key_name(n, src)?]))
        }
        Kind::Pipe(p, f) => {
            let mut keys = Vec::new();
            top_fields(f, src, &mut keys)?;
            Some((prefix_of(p), keys))
        }
        Kind::Comma(..) => {
            let mut keys = Vec::new();
            top_fields(n, src, &mut keys)?;
            Some((None, keys))
        }
        _ => None,
    }
}

fn prefix_of(base: &Node) -> Option<&Node> {
    (!matches!(base.kind, Kind::Identity)).then_some(base)
}

/// Collect `.a, .b, ."c"` — single fields on `.` — as JSON string literals.
fn top_fields(n: &Node, src: &str, out: &mut Vec<String>) -> Option<()> {
    match &n.kind {
        Kind::Paren(x) => top_fields(x, src, out),
        Kind::Comma(a, b) => {
            top_fields(a, src, out)?;
            top_fields(b, src, out)
        }
        Kind::Field { base, .. } | Kind::Index { base, .. }
            if matches!(base.kind, Kind::Identity) =>
        {
            out.push(key_name(n, src)?);
            Some(())
        }
        _ => None,
    }
}

/// The key a single field or string-index step names, as a string literal.
fn key_name(n: &Node, src: &str) -> Option<String> {
    let lit = |s: &super::StrLit| -> Option<String> {
        plain_string(s).then(|| {
            let mut t = String::from("\"");
            for seg in &s.parts {
                if let super::StrSeg::Text(sp) = seg {
                    t.push_str(&src[sp.start..sp.end]);
                }
            }
            t.push('"');
            t
        })
    };
    match &n.kind {
        Kind::Field {
            name: super::FieldName::Ident(f),
            ..
        } => Some(format!("\"{f}\"")),
        Kind::Field {
            name: super::FieldName::Str(s),
            ..
        } => lit(s),
        Kind::Index { index, .. } => match &index.kind {
            Kind::Str(s) => lit(s),
            _ => None,
        },
        _ => None,
    }
}

/// A prefix jaq's `|=` walks as jq's `_modify` would: one pass, no `..`
/// (whose update order differs from jq's paths-first evaluation).
fn update_prefix(n: &Node) -> bool {
    single_path(n) && !contains_recurse(n)
}

fn contains_recurse(n: &Node) -> bool {
    matches!(n.kind, Kind::Recurse)
        || matches!(&n.kind, Kind::Call { name, args } if name == "recurse" && args.is_empty())
        || n.children().into_iter().any(contains_recurse)
}

/// `.a`, `.a.b`, `.a, .b.c`: paths through field names alone.
fn fields_only(node: &Node) -> bool {
    match &node.kind {
        Kind::Identity => true,
        Kind::Field { base, name } => plain_field(name) && fields_only(base),
        Kind::Paren(n) => fields_only(n),
        Kind::Comma(a, b) => fields_only(a) && fields_only(b),
        _ => false,
    }
}

fn plain_field(name: &super::FieldName) -> bool {
    match name {
        super::FieldName::Ident(_) => true,
        super::FieldName::Str(s) => plain_string(s),
    }
}

fn plain_string(s: &super::StrLit) -> bool {
    s.format.is_none() && s.parts.iter().all(|p| matches!(p, super::StrSeg::Text(_)))
}

/// A number or plain string literal, possibly negated.
fn literal_index(n: &Node) -> bool {
    match &n.kind {
        Kind::Number => true,
        Kind::Str(s) => plain_string(s),
        Kind::Neg(inner) => matches!(inner.kind, Kind::Number),
        _ => false,
    }
}

/// Whether an expression produces at most one value per input.
fn single_valued(node: &Node) -> bool {
    match &node.kind {
        Kind::Identity | Kind::Number | Kind::Var(_) | Kind::Loc | Kind::Array(_) => true,
        Kind::Str(s) => s.parts.iter().all(|p| match p {
            super::StrSeg::Text(_) => true,
            super::StrSeg::Interp(q) => single_valued(q),
        }),
        Kind::Field { base, name } => single_valued(base) && plain_field(name),
        Kind::Index { base, index } => single_valued(base) && single_valued(index),
        Kind::Neg(n) | Kind::Paren(n) | Kind::Optional(n) => single_valued(n),
        Kind::Pipe(a, b) => single_valued(a) && single_valued(b),
        Kind::Binary { op, lhs, rhs } => {
            !is_assignment(op) && single_valued(lhs) && single_valued(rhs)
        }
        Kind::Object(pairs) => pairs.iter().all(|p| match p {
            super::ObjPair::KeyValue { key, value } => {
                single_valued(value)
                    && match key {
                        super::ObjKey::Ident(_) | super::ObjKey::Var(_) => true,
                        super::ObjKey::Str(s) => plain_string(s),
                        super::ObjKey::Expr(k) => single_valued(k),
                    }
            }
            super::ObjPair::Shorthand(super::ObjShort::Str(s)) => plain_string(s),
            super::ObjPair::Shorthand(_) => true,
        }),
        Kind::If {
            branches,
            otherwise,
        } => {
            branches
                .iter()
                .all(|(c, t)| single_valued(c) && single_valued(t))
                && otherwise.as_deref().is_none_or(single_valued)
        }
        Kind::Call { name, args } => match (name.as_str(), args.len()) {
            // The first group collapses its arguments to one value whatever
            // they yield; the second takes none.
            ("any" | "all" | "isempty", _) | ("IN", 1 | 2) => args.iter().all(repeatable),
            (
                "not" | "length" | "type" | "ascii_downcase" | "ascii_upcase" | "tostring"
                | "tonumber" | "keys" | "isnan" | "utf8bytelength" | "tojson" | "floor" | "abs",
                0,
            ) => true,
            (
                "has" | "test" | "startswith" | "endswith" | "contains" | "inside" | "in"
                | "ltrimstr" | "rtrimstr",
                _,
            ) => args.iter().all(single_valued),
            _ => false,
        },
        _ => false,
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

/// Whether evaluating `n` twice is unobservable. The `del` fallback evaluates
/// its target a second time on the error path, so a target that reads input or
/// writes to stderr must not take it.
fn repeatable(n: &Node) -> bool {
    let effectful = matches!(&n.kind, Kind::Call { name, .. }
        if matches!(name.as_str(), "input" | "inputs" | "debug" | "stderr" | "halt" | "halt_error" | "input_line_number" | "now"));
    !effectful && n.children().into_iter().all(repeatable)
}

/// Whether an assignment target spells out several paths with a comma.
fn has_comma(node: &Node) -> bool {
    matches!(node.kind, Kind::Comma(..)) || node.children().into_iter().any(has_comma)
}

fn is_assignment(op: &str) -> bool {
    matches!(op, "=" | "|=" | "+=" | "-=" | "*=" | "/=" | "%=" | "//=")
}

/// A `reduce`/`foreach` source jaq would not parse as written: anything that
/// is not a single postfix term.
const fn is_compound(source: &Node) -> bool {
    matches!(source.kind, Kind::Binary { .. } | Kind::Neg(_))
}

#[cfg(test)]
mod tests {
    use super::rewrite;

    fn rw(src: &str) -> Option<String> {
        rewrite(src).map(|r| r.text)
    }

    #[test]
    fn programs_without_assignment_are_not_touched() {
        assert_eq!(rw(".a | select(.b == 1)"), None);
        assert_eq!(rw("[.[] | . + 1]"), None);
        // `==`, `<=`, `>=`, `!=` are comparisons, not assignments.
        assert_eq!(rw(".a == 1 and .b <= 2 or .c != 3"), None);
    }

    #[test]
    fn a_literal_target_is_prepared_inline_then_assigned_natively() {
        let out = rw(".a.b = 1").expect("rewritten");
        // The right-hand side is bound first, on the original input.
        assert!(out.starts_with("((1) as $__pf_t0 | "), "{out}");
        // jq's own message for a container of the wrong type, key included.
        assert!(
            out.contains(r#"error("Cannot index \(type) with string \"" + "b" + "\"")"#),
            "{out}"
        );
        assert!(out.ends_with("| (.a.b) = $__pf_t0)"), "{out}");
        assert!(
            !out.contains("_pf_assign"),
            "no per-path prelude call: {out}"
        );
    }

    #[test]
    fn an_update_runs_jaqs_operator_after_the_guard() {
        let out = rw(".a |= . + 1").expect("rewritten");
        assert!(out.ends_with("| (.a) |= (. + 1))"), "{out}");
    }

    #[test]
    fn a_target_naming_nothing_new_needs_no_guard() {
        assert_eq!(rw(".[] |= . + 1").as_deref(), Some("((.[]) |= (. + 1))"));
    }

    #[test]
    fn a_comma_target_goes_through_jqs_modify() {
        // jaq's update deletes `.[1,2]` one after the other, shifting the second.
        assert_eq!(
            rw(".[1,2] += 1").as_deref(),
            Some("((1) as $__pf_t0 | _pf_modify((.[1,2]); . + $__pf_t0))")
        );
    }

    #[test]
    fn arithmetic_update_binds_the_rhs_first() {
        let out = rw(".a += 1").expect("rewritten");
        assert!(out.starts_with("((1) as $__pf_t0 | "), "{out}");
        assert!(out.ends_with("| (.a) += $__pf_t0)"), "{out}");
        let out = rw(".a //= 2").expect("rewritten");
        assert!(out.starts_with("((2) as $__pf_t0 | "), "{out}");
        assert!(out.ends_with("| (.a) //= $__pf_t0)"), "{out}");
    }

    #[test]
    fn surrounding_text_is_kept_byte_for_byte() {
        // Odd spacing and the comment survive; only the assignment changed.
        let out = rw("[ .x ,  (.a = 1) ]  # keep me").expect("rewritten");
        assert!(out.starts_with("[ .x ,  (((1) as $__pf_t0 | "), "{out}");
        assert!(out.ends_with("(.a) = $__pf_t0)) ]  # keep me"), "{out}");
    }

    #[test]
    fn del_with_one_path_per_array_uses_jaqs_own_del() {
        let out = rw("del(.[] | select(.x))").expect("rewritten");
        assert!(out.contains("_pf_del0(.[] | select(.x))"), "{out}");
        // ...with jq's exact algorithm behind it for errors and the root.
        assert!(
            out.contains("catch ($__pf_t0 | _pf_delpaths([path(.[] | select(.x))]))"),
            "{out}"
        );
    }

    #[test]
    fn del_of_named_keys_keeps_key_order() {
        let out = rw("del(.[].a)").expect("rewritten");
        assert!(out.contains("((.[]) |= (if type == \"object\""), "{out}");
        assert!(out.contains("keys_unsorted"), "{out}");
    }

    #[test]
    fn del_of_an_index_checks_for_an_array() {
        let out = rw("del(.a[0])").expect("rewritten");
        assert!(out.contains("((.a) |= _pf_delat(.[0]))"), "{out}");
    }

    #[test]
    fn del_with_several_paths_into_one_array_is_left_to_the_repair() {
        // jaq would delete `.[1]` against the array `.[0]` already shortened.
        assert_eq!(rw("del(.[0,1])"), None);
        assert_eq!(
            rw("del(.[] | select(. == (1, 2)))"),
            None,
            "select may emit twice"
        );
    }

    #[test]
    fn a_del_target_reading_input_is_not_routed_through_the_fallback() {
        // The fallback would evaluate the target, and so `input`, twice.
        assert_eq!(rw("del(.[] | select(any(input; . == 1)))"), None);
        assert!(rw("del(.[] | select(any(.[]; . == 1)))").is_some());
    }

    #[test]
    fn a_program_defining_select_keeps_its_del() {
        assert_eq!(rw("def select(f): .; del(.[] | select(.x))"), None);
    }

    #[test]
    fn update_with_empty_is_deletion() {
        let out = rw(".[0] |= empty").expect("rewritten");
        assert!(out.contains("_pf_delat(.[0])"), "{out}");
    }

    #[test]
    fn a_rewritten_binding_cannot_swallow_what_follows() {
        // Without the outer parentheses, `R as $t | …` would take `, 2` along.
        let out = rw("[.a += 1, 2]").expect("rewritten");
        assert!(out.starts_with("[((1) as $__pf_t0 |"), "{out}");
        assert!(out.ends_with("), 2]"), "{out}");
    }

    #[test]
    fn nested_assignments_get_distinct_temporaries() {
        let out = rw(".a += (.b += 1)").expect("rewritten");
        assert!(
            out.contains("$__pf_t0") && out.contains("$__pf_t1"),
            "{out}"
        );
    }

    #[test]
    fn assignments_inside_strings_and_defs_are_found() {
        assert!(rw("\"\\(.a = 1)\"").is_some());
        assert!(rw("def f: .a = 1; f").is_some());
        assert!(rw("{k: (.a |= 2)}").is_some());
    }

    #[test]
    fn compound_reduce_sources_are_parenthesised() {
        assert_eq!(
            rw("reduce .[] / .[] as $i (0; . + $i)").as_deref(),
            Some("reduce (.[] / .[]) as $i (0; . + $i)")
        );
        assert_eq!(
            rw("reduce .[] as $i (0; . + $i)"),
            None,
            "a simple source is left alone"
        );
    }

    #[test]
    fn unparseable_programs_pass_through() {
        assert_eq!(rw(".a = "), None);
    }
}
