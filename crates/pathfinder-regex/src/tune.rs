// SPDX-FileCopyrightText: 2002-2024 K.Kosako
// SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
// SPDX-License-Identifier: BSD-2-Clause AND (GPL-3.0-or-later OR MIT)

//! What Oniguruma does to the tree between parsing and compiling that
//! changes what it accepts or matches (`tune_tree` in `src/regcomp.c`):
//! backreferences checked against the groups, case-insensitive strings
//! unravelled into the strings and classes they match
//! (`unravel_case_fold_string`), and lookbehinds checked and measured
//! (`tune_look_behind`).

use std::cell::RefCell;

use crate::ast::{Anchor, Class, Node};
use crate::enc;
use crate::error::{Error, ErrorKind as E};
use crate::unicode;

/// `LOOK_BEHIND_MAX_CHAR_LEN` (`STEP_BACK_MAX_CHAR_LEN`).
const LOOK_BEHIND_MAX_CHAR_LEN: u32 = 65535;

/// Tunes the tree in place (`tune_tree`, in its order, so the first error
/// is Oniguruma's).
pub fn tune(root: &mut Node, groups: usize) -> Result<(), Error> {
    let caps = collect_captures(root);
    let mut backrefed = Vec::new();
    collect_backrefs(root, &mut backrefed);
    // `check_backrefs`, over the whole tree before `tune_tree`.
    if backrefed.iter().any(|&g| g > groups) {
        return Err(Error::new(E::InvalidBackref));
    }
    Tuner {
        caps: RefCell::new(caps),
        backrefed,
        measuring: RefCell::new(Vec::new()),
    }
    .tune(root, false)
}

struct Tuner {
    /// Each capture group's body as tuned so far, for a backreference's
    /// length: a group after the lookbehind is still untuned there.
    caps: RefCell<Vec<Option<Node>>>,
    /// The groups some backreference names (`ND_IS_BACKREF`).
    backrefed: Vec<usize>,
    /// The groups whose length is being measured through a backreference.
    measuring: RefCell<Vec<usize>>,
}

fn collect_backrefs(n: &Node, out: &mut Vec<usize>) {
    if let Node::Backref { groups, .. } | Node::BackrefCheck { groups } = n {
        out.extend(groups);
    }
    for c in children(n) {
        collect_backrefs(c, out);
    }
}

/// Every capture group's body, by number (index 0 unused).
fn collect_captures(root: &Node) -> Vec<Option<Node>> {
    fn walk(n: &Node, out: &mut Vec<Option<Node>>) {
        if let Node::Capture { num, body } = n {
            if out.len() <= *num {
                out.resize(num + 1, None);
            }
            out[*num] = Some((**body).clone());
        }
        for c in children(n) {
            walk(c, out);
        }
    }
    let mut out = vec![None];
    walk(root, &mut out);
    out
}

fn children(n: &Node) -> Vec<&Node> {
    match n {
        Node::List(xs) | Node::Alt(xs) => xs.iter().collect(),
        Node::Look { body, .. }
        | Node::Quant { body, .. }
        | Node::Atomic(body)
        | Node::Capture { body, .. }
        | Node::Group(body) => vec![body],
        Node::IfElse { cond, then, els } => {
            let mut v: Vec<&Node> = vec![cond];
            v.extend(then.as_deref());
            v.extend(els.as_deref());
            v
        }
        _ => Vec::new(),
    }
}

impl Tuner {
    fn tune(&self, n: &mut Node, in_lb: bool) -> Result<(), Error> {
        match n {
            Node::List(xs) => {
                for i in 0..xs.len() {
                    self.tune(&mut xs[i], in_lb)?;
                    if in_lb && i > 0 {
                        let (head, tail) = xs.split_at_mut(i);
                        tune_next(&mut head[i - 1], &tail[0]);
                    }
                }
            }
            Node::Alt(xs) => {
                for x in xs {
                    self.tune(x, in_lb)?;
                }
            }
            Node::Str { bytes, icase } => {
                if *icase {
                    *n = unravel(bytes, in_lb);
                }
            }
            Node::Capture { num, body } => {
                self.tune(body, in_lb)?;
                self.caps.borrow_mut()[*num] = Some((**body).clone());
            }
            Node::Group(body)
            | Node::Atomic(body)
            | Node::Quant { body, .. }
            | Node::Look {
                behind: false,
                body,
                ..
            } => self.tune(body, in_lb)?,
            Node::IfElse { cond, then, els } => {
                self.tune(cond, in_lb)?;
                if let Some(t) = then {
                    self.tune(t, in_lb)?;
                }
                if let Some(e) = els {
                    self.tune(e, in_lb)?;
                }
            }
            Node::Look { behind: true, .. } => self.look_behind(n)?,
            _ => {}
        }
        Ok(())
    }

    /// `tune_look_behind`.
    fn look_behind(&self, n: &mut Node) -> Result<(), Error> {
        let Node::Look { not, body, .. } = n else {
            unreachable!("a lookbehind")
        };
        let not = *not;
        let mut used = false;
        if !allowed_in_look_behind(body, not, &mut used, &self.backrefed) {
            return Err(Error::new(E::InvalidLookBehindPattern));
        }
        self.tune(body, true)?;
        // A list whose every item reduces away makes Oniguruma's tuning
        // return 1, which jq reports as an error with no message.
        if alt_reduce(body) != 0 {
            return Err(Error::new(E::UndefinedErrorCode));
        }
        let (len, top_alt_fixed) = self.char_len(body, 0)?;
        if len.max.is_some_and(|m| m > LOOK_BEHIND_MAX_CHAR_LEN)
            || len.min > LOOK_BEHIND_MAX_CHAR_LEN
        {
            return Err(Error::new(E::InvalidLookBehindPattern));
        }
        if len.min == 0 && len.min_is_sure && !used {
            // `onig_node_reset_empty` / `onig_node_reset_fail`.
            *n = if not { fail() } else { Node::Empty };
            return Ok(());
        }
        // Perl_NG allows neither alternatives of different lengths
        // (`ONIG_SYN_DIFFERENT_LEN_ALT_LOOK_BEHIND`) nor a variable length
        // (`ONIG_SYN_VARIABLE_LEN_LOOK_BEHIND`).
        if top_alt_fixed || len.max != Some(len.min) {
            return Err(Error::new(E::InvalidLookBehindPattern));
        }
        let Node::Look { body, not, .. } = std::mem::replace(n, Node::Empty) else {
            unreachable!("a lookbehind")
        };
        *n = Node::LookBehind {
            not,
            chars: len.min,
            body,
        };
        Ok(())
    }

    /// `node_char_len1`: the length of what `n` matches, in characters, and
    /// whether it is the top level's alternatives that differ.
    #[expect(
        clippy::too_many_lines,
        reason = "Oniguruma's one switch over the node kinds; kept whole to stay comparable"
    )]
    fn char_len(&self, n: &Node, level: u32) -> Result<(CharLen, bool), Error> {
        let level = level + 1;
        Ok(match n {
            Node::List(xs) => {
                let mut ci = CharLen::fixed(0);
                for (i, x) in xs.iter().enumerate() {
                    let (t, _) = self.char_len(x, level)?;
                    ci = if i == 0 { t } else { ci.add(t) };
                }
                (ci, false)
            }
            Node::Alt(xs) => {
                let (mut ci, _) = self.char_len(&xs[0], level)?;
                let mut fixed = true;
                for x in &xs[1..] {
                    let (t, _) = self.char_len(x, level)?;
                    if !t.is_fixed() {
                        fixed = false;
                    }
                    ci = ci.alt(t);
                }
                if ci.is_fixed() {
                    (ci, false)
                } else {
                    (ci, fixed && level == 1)
                }
            }
            // A case-insensitive string not unravelled yet: a group the
            // lookbehind refers to before it is tuned (`/(?i)(?<=\1)(a)/`).
            Node::Str { icase: true, .. } => {
                return Err(Error::new(E::InvalidLookBehindPattern));
            }
            Node::Str { bytes, .. } => {
                let mut clen = 0;
                let mut i = 0;
                while i < bytes.len() {
                    i += enc::len(bytes, i);
                    clen += 1;
                }
                (CharLen::fixed(clen), false)
            }
            Node::Quant {
                body, lower, upper, ..
            } => {
                if Some(*lower) == *upper {
                    if *lower == 0 {
                        (CharLen::fixed(0), false)
                    } else {
                        let (ci, _) = self.char_len(body, level)?;
                        (ci.times(*lower, Some(*lower)), false)
                    }
                } else {
                    let (ci, _) = self.char_len(body, level)?;
                    (ci.times(*lower, *upper), false)
                }
            }
            Node::Class(_) | Node::Word { .. } | Node::Any { .. } => (CharLen::fixed(1), false),
            Node::Capture { body, .. } => {
                let (mut ci, _) = self.char_len(body, level)?;
                // "can't optimize look-behind if capture exists."
                ci.min_is_sure = false;
                (ci, false)
            }
            Node::Group(body) | Node::Atomic(body) => self.char_len(body, level)?,
            Node::IfElse { cond, then, els } => {
                let (mut ci, _) = self.char_len(cond, level)?;
                if let Some(t) = then {
                    ci = ci.add(self.char_len(t, level)?.0);
                }
                let eci = match els {
                    Some(e) => self.char_len(e, level)?.0,
                    None => CharLen::fixed(0),
                };
                (ci.alt(eci), false)
            }
            Node::Anchor(_)
            | Node::Look { .. }
            | Node::LookBehind { .. }
            | Node::BackrefCheck { .. } => {
                let mut ci = CharLen::fixed(0);
                ci.min_is_sure = false;
                (ci, false)
            }
            Node::Backref {
                groups,
                level: nest,
                recursive,
                ..
            } => {
                if *recursive {
                    let ci = CharLen {
                        min: 0,
                        max: if nest.is_some() { None } else { Some(0) },
                        min_is_sure: false,
                    };
                    (ci, false)
                } else {
                    let mut ci: Option<CharLen> = None;
                    for &g in groups {
                        let caps = self.caps.borrow();
                        let body = caps.get(g).and_then(Option::as_ref);
                        let mut t = match body {
                            // A group already being measured (`MARK1`).
                            Some(_) if self.measuring.borrow().contains(&g) => CharLen {
                                min: 0,
                                max: None,
                                min_is_sure: false,
                            },
                            Some(b) => {
                                self.measuring.borrow_mut().push(g);
                                let r = self.char_len(b, level);
                                self.measuring.borrow_mut().pop();
                                r?.0
                            }
                            None => CharLen::fixed(0),
                        };
                        // The group's node is a capture, which is never
                        // sure of its minimum.
                        t.min_is_sure = false;
                        ci = Some(match ci {
                            None => t,
                            Some(c) => c.alt(t),
                        });
                    }
                    (ci.unwrap_or(CharLen::fixed(0)), false)
                }
            }
            Node::Empty | Node::Keep | Node::Fail => (CharLen::fixed(0), false),
        })
    }
}

/// A length in characters (`MinMaxCharLen`); `max` `None` is unbounded.
#[derive(Clone, Copy, Debug)]
struct CharLen {
    min: u32,
    max: Option<u32>,
    min_is_sure: bool,
}

impl CharLen {
    const fn fixed(n: u32) -> Self {
        Self {
            min: n,
            max: Some(n),
            min_is_sure: true,
        }
    }

    fn is_fixed(self) -> bool {
        self.max == Some(self.min)
    }

    fn add(self, o: Self) -> Self {
        Self {
            min: self.min.saturating_add(o.min),
            max: self.max.zip(o.max).map(|(a, b)| a.saturating_add(b)),
            min_is_sure: self.min_is_sure && o.min_is_sure,
        }
    }

    fn alt(self, o: Self) -> Self {
        let (min, min_is_sure) = match self.min.cmp(&o.min) {
            std::cmp::Ordering::Greater => (o.min, o.min_is_sure),
            std::cmp::Ordering::Equal => (self.min, self.min_is_sure || o.min_is_sure),
            std::cmp::Ordering::Less => (self.min, self.min_is_sure),
        };
        let max = match (self.max, o.max) {
            (Some(a), Some(b)) => Some(a.max(b)),
            _ => None,
        };
        Self {
            min,
            max,
            min_is_sure,
        }
    }

    fn times(self, lower: u32, upper: Option<u32>) -> Self {
        Self {
            min: self.min.saturating_mul(lower),
            max: match upper {
                None => None,
                Some(u) => self.max.map(|m| m.saturating_mul(u)),
            },
            min_is_sure: self.min_is_sure,
        }
    }
}

/// `check_node_in_look_behind`: whether `n` may stand in a lookbehind,
/// noting in `used` a capture group or `\K` that keeps a zero-length one.
fn allowed_in_look_behind(n: &Node, not: bool, used: &mut bool, backrefed: &[usize]) -> bool {
    let mut ok = |x: &Node| allowed_in_look_behind(x, not, used, backrefed);
    match n {
        Node::List(xs) | Node::Alt(xs) => xs.iter().all(ok),
        Node::Quant { body, .. } | Node::Group(body) | Node::Atomic(body) => ok(body),
        Node::Capture { num, body } => {
            if not || !ok(body) {
                return false;
            }
            if backrefed.contains(num) {
                *used = true;
            }
            true
        }
        Node::IfElse { cond, then, els } => {
            ok(cond) && then.as_deref().is_none_or(&mut ok) && els.as_deref().is_none_or(&mut ok)
        }
        Node::Anchor(a) => matches!(
            a,
            Anchor::BeginLine
                | Anchor::EndLine
                | Anchor::BeginBuf
                | Anchor::BeginPosition
                | Anchor::WordBoundary
                | Anchor::NoWordBoundary
                | Anchor::TextSegmentBoundary
                | Anchor::NoTextSegmentBoundary
        ),
        Node::Look {
            behind: true,
            not: inner,
            body,
        } => (!*inner || not) && ok(body),
        Node::Look { behind: false, .. } => false,
        Node::Keep => {
            *used = true;
            true
        }
        _ => true,
    }
}

/// `alt_reduce_in_look_behind`: in a lookbehind, a repeated string, class
/// or backreference repeats exactly its lower bound, and none at all when
/// that is zero.
fn alt_reduce(n: &mut Node) -> i32 {
    match n {
        Node::Alt(xs) => {
            let mut r = 0;
            for x in xs {
                r = list_reduce(x);
                if r != 0 {
                    break;
                }
            }
            r
        }
        n => list_reduce(n),
    }
}

/// `list_reduce_in_look_behind`; 0 to go on.
fn list_reduce(n: &mut Node) -> i32 {
    match n {
        Node::Quant { .. } => {
            node_reduce(n);
            0
        }
        Node::List(xs) => {
            let mut r = 0;
            for x in xs {
                r = node_reduce(x);
                if r <= 0 {
                    break;
                }
            }
            r
        }
        _ => 0,
    }
}

/// `node_reduce_in_look_behind`; 1 when the node was removed.
fn node_reduce(n: &mut Node) -> i32 {
    if let Node::Quant {
        body, lower, upper, ..
    } = n
        && matches!(
            **body,
            // `(?:)` is an empty string node in Oniguruma.
            Node::Empty
                | Node::Str { .. }
                | Node::Class(_)
                | Node::Word { .. }
                | Node::Any { .. }
                | Node::Backref { .. }
        )
    {
        *upper = Some(*lower);
        if *lower == 0 {
            return 1;
        }
    }
    0
}

/// `tune_next`'s automatic possessivisation, `a*b` => `(?>a*)b`: a greedy
/// unbounded repeat of something that cannot begin what follows. It keeps
/// what matches; in a lookbehind it keeps the repeat from being reduced
/// (`(?<=b*)` is valid, `(?<=b* )` is not). Done there only.
fn tune_next(node: &mut Node, next: &Node) {
    let mut node = node;
    while let Node::Capture { body, .. } = node {
        node = body;
    }
    let Node::Quant {
        body,
        lower,
        upper: None,
        greedy: true,
    } = node
    else {
        return;
    };
    if *lower > 1 || !is_strict_real_node(body) {
        return;
    }
    let (Some(x), Some(y)) = (head_literal(body), head_literal(next)) else {
        return;
    };
    if is_exclusive(x, y) {
        let q = std::mem::replace(node, Node::Empty);
        *node = Node::Atomic(Box::new(q));
    }
}

/// `is_strict_real_node`.
fn is_strict_real_node(n: &Node) -> bool {
    match n {
        Node::Str { bytes, .. } => !bytes.is_empty(),
        Node::Class(_) | Node::Word { .. } | Node::Any { .. } => true,
        _ => false,
    }
}

/// `get_tree_head_literal` with `exact` 0: what a match of `n` begins with,
/// when it is a string, a class or a word type.
fn head_literal(n: &Node) -> Option<&Node> {
    match n {
        Node::Word { .. } | Node::Class(_) => Some(n),
        Node::Str { bytes, .. } if !bytes.is_empty() => Some(n),
        Node::List(xs) => xs.first().and_then(head_literal),
        Node::Quant { body, lower, .. } if *lower > 0 => head_literal(body),
        Node::Group(body)
        | Node::Capture { body, .. }
        | Node::Atomic(body)
        | Node::Look {
            behind: false,
            not: false,
            body,
        } => head_literal(body),
        _ => None,
    }
}

/// `is_exclusive`: whether nothing can begin with both `x` and `y`.
fn is_exclusive(x: &Node, y: &Node) -> bool {
    match (x, y) {
        (Node::Word { not: a }, Node::Word { not: b }) => a != b,
        (Node::Word { .. }, Node::Class(_) | Node::Str { .. })
        | (Node::Class(_), Node::Str { .. }) => is_exclusive(y, x),
        (Node::Class(xc), Node::Word { not }) => {
            if !xc.ranges.is_empty() || xc.not {
                return false;
            }
            if *not {
                (0..256u32).all(|i| !xc.bit(i) || unicode::is_word(i))
            } else {
                (0..256u32).all(|i| !xc.bit(i) || !unicode::is_word(i))
            }
        }
        (Node::Class(xc), Node::Class(yc)) => {
            for i in 0..256u32 {
                if xc.bit(i) != xc.not && yc.bit(i) != yc.not {
                    return false;
                }
            }
            (xc.ranges.is_empty() && !xc.not) || (yc.ranges.is_empty() && !yc.not)
        }
        (Node::Str { bytes, .. }, Node::Word { not }) => {
            if unicode::is_word(enc::code(bytes, 0)) {
                *not
            } else {
                !*not
            }
        }
        (Node::Str { bytes, .. }, Node::Class(cc)) => cc.holds_code(enc::code(bytes, 0)) == cc.not,
        (Node::Str { bytes: a, .. }, Node::Str { bytes: b, .. }) => {
            a.iter().zip(b.iter()).any(|(p, q)| p != q)
        }
        _ => false,
    }
}

/// A node that never matches (`onig_node_reset_fail`).
fn fail() -> Node {
    Node::Fail
}

/// One alternative of a case-folded string (`OnigCaseFoldCodeItem`): the
/// pattern bytes it stands for and the code points it is.
struct Item {
    byte_len: usize,
    codes: Vec<u32>,
}

/// `onigenc_unicode_get_case_fold_codes_by_str` at `p`.
#[expect(
    clippy::too_many_lines,
    clippy::many_single_char_names,
    reason = "Oniguruma's function and names, kept to stay comparable"
)]
fn case_fold_codes_by_str(s: &[u8], p: usize) -> Vec<Item> {
    let mut items = Vec::new();
    let mut orig = [0u32; 3];
    let mut lens = [0usize; 3];
    let mut codes = [0u32; 3];
    orig[0] = enc::code(s, p);
    lens[0] = enc::len(s, p);
    let buk1 = unicode::unfold_key(orig[0]);
    codes[0] = single_fold(orig[0]);
    let mut q = p + lens[0];
    // The variants of a fold target: itself, then what folds to it.
    let variants = |c: u32| -> Vec<u32> {
        let mut v = vec![c];
        if let Some(us) = unicode::unfolds1(c) {
            v.extend(us);
        }
        v
    };
    if q < s.len() {
        orig[1] = enc::code(s, q);
        lens[1] = lens[0] + enc::len(s, q);
        codes[1] = single_fold(orig[1]);
        q += enc::len(s, q);
        if q < s.len() {
            orig[2] = enc::code(s, q);
            lens[2] = lens[1] + enc::len(s, q);
            codes[2] = single_fold(orig[2]);
            if let Some(us) = unicode::unfolds_n(&codes) {
                for &u in us {
                    items.push(Item {
                        byte_len: lens[2],
                        codes: vec![u],
                    });
                }
                let cs: Vec<Vec<u32>> = codes.iter().map(|&c| variants(c)).collect();
                for &a in &cs[0] {
                    for &b in &cs[1] {
                        for &c in &cs[2] {
                            if [a, b, c] == orig {
                                continue;
                            }
                            items.push(Item {
                                byte_len: lens[2],
                                codes: vec![a, b, c],
                            });
                        }
                    }
                }
                return items;
            }
        }
        if let Some(us) = unicode::unfolds_n(&codes[..2]) {
            for &u in us {
                items.push(Item {
                    byte_len: lens[1],
                    codes: vec![u],
                });
            }
            let cs: Vec<Vec<u32>> = codes[..2].iter().map(|&c| variants(c)).collect();
            for &a in &cs[0] {
                for &b in &cs[1] {
                    if [a, b] == orig[..2] {
                        continue;
                    }
                    items.push(Item {
                        byte_len: lens[1],
                        codes: vec![a, b],
                    });
                }
            }
            return items;
        }
    }
    match buk1 {
        Some(f) if f.as_slice().len() == 1 => {
            let k = f.as_slice()[0];
            items.push(Item {
                byte_len: lens[0],
                codes: vec![k],
            });
            for &u in unicode::unfolds1(k).unwrap_or(&[]) {
                if u != orig[0] {
                    items.push(Item {
                        byte_len: lens[0],
                        codes: vec![u],
                    });
                }
            }
        }
        Some(f) => {
            let seq = f.as_slice();
            for &u in unicode::unfolds_n(seq).unwrap_or(&[]) {
                if u != orig[0] {
                    items.push(Item {
                        byte_len: lens[0],
                        codes: vec![u],
                    });
                }
            }
            let cs: Vec<Vec<u32>> = seq.iter().map(|&c| variants(c)).collect();
            let mut combos: Vec<Vec<u32>> = vec![Vec::new()];
            for set in &cs {
                combos = combos
                    .into_iter()
                    .flat_map(|pre| {
                        set.iter().map(move |&c| {
                            let mut v = pre.clone();
                            v.push(c);
                            v
                        })
                    })
                    .collect();
            }
            for c in combos {
                items.push(Item {
                    byte_len: lens[0],
                    codes: c,
                });
            }
        }
        None => {
            for &u in unicode::unfolds1(orig[0]).unwrap_or(&[]) {
                items.push(Item {
                    byte_len: lens[0],
                    codes: vec![u],
                });
            }
        }
    }
    items
}

/// The one code point `c` folds to, or `c` (`unfold_key` with
/// `fold_len == 1`).
fn single_fold(c: u32) -> u32 {
    match unicode::unfold_key(c) {
        Some(f) if f.as_slice().len() == 1 => f.as_slice()[0],
        _ => c,
    }
}

/// `unravel_case_fold_string`: a case-insensitive string as the exact
/// strings, classes and alternatives it matches.
fn unravel(s: &[u8], in_look_behind: bool) -> Node {
    let mut list: Vec<Node> = Vec::new();
    let mut p = 0;
    while p < s.len() {
        let items = case_fold_codes_by_str(s, p);
        let one = enc::len(s, p);
        if items.is_empty() {
            push_str(&mut list, &s[p..(p + one).min(s.len())]);
            p += one;
            continue;
        }
        if in_look_behind {
            // `unravel_cf_look_behind_add`: one-for-one folds only.
            let items = if items[0].byte_len == one {
                items
            } else {
                case_fold_codes_by_str(&s[..p + one], p)
            };
            let singles: Vec<u32> = items
                .iter()
                .filter(|i| i.byte_len == one && i.codes.len() == 1)
                .map(|i| i.codes[0])
                .collect();
            if singles.is_empty() {
                push_str(&mut list, &s[p..p + one]);
            } else {
                let mut cc = Class::default();
                cc.add_code(enc::code(s, p));
                for c in singles {
                    cc.add_code(c);
                }
                list.push(Node::Class(Box::new(cc)));
            }
            p += one;
            continue;
        }
        let q = p + items[0].byte_len;
        if items.iter().all(|i| i.codes.len() == 1) {
            let mut cc = Class::default();
            cc.add_code(enc::code(s, p));
            for i in &items {
                cc.add_code(i.codes[0]);
            }
            list.push(Node::Class(Box::new(cc)));
        } else {
            let mut alts = vec![Node::Str {
                bytes: s[p..q].to_vec(),
                icase: false,
            }];
            for i in &items {
                let mut b = Vec::new();
                for &c in &i.codes {
                    enc::push_code(c, &mut b);
                }
                alts.push(Node::Str {
                    bytes: b,
                    icase: false,
                });
            }
            list.push(Node::Alt(alts));
        }
        p = q;
    }
    if list.len() == 1 {
        list.remove(0)
    } else {
        Node::List(list)
    }
}

/// Appends exact bytes, joining them to an exact string just before.
fn push_str(list: &mut Vec<Node>, b: &[u8]) {
    if let Some(Node::Str {
        bytes,
        icase: false,
    }) = list.last_mut()
    {
        bytes.extend_from_slice(b);
    } else {
        list.push(Node::Str {
            bytes: b.to_vec(),
            icase: false,
        });
    }
}
