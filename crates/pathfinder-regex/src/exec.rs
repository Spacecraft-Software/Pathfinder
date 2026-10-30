// SPDX-FileCopyrightText: 2002-2024 K.Kosako
// SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
// SPDX-License-Identifier: BSD-2-Clause AND (GPL-3.0-or-later OR MIT)

//! Running a [`Program`]: Oniguruma's backtracking matcher (`match_at`
//! and `onig_search` in `src/regexec.c`), its stack discipline kept: what
//! is undone on backtracking, what an atomic group voids, where an empty
//! iteration leaves a loop, and how the longest match is chosen.

use crate::ast::{Anchor, Class};
use crate::compile::{Op, Program};
use crate::enc;
use crate::error::{Error, ErrorKind as E};
use crate::tables::Egcb;
use crate::unicode;

/// `DEFAULT_RETRY_LIMIT_IN_MATCH`: backtracks allowed from one start.
const RETRY_LIMIT_IN_MATCH: u64 = 10_000_000;

/// The search options jq passes (`ONIG_OPTION_FIND_*`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SearchOptions {
    /// `ONIG_OPTION_FIND_LONGEST`: the longest match of all starts.
    pub longest: bool,
    /// `ONIG_OPTION_FIND_NOT_EMPTY`: an empty match does not count.
    pub not_empty: bool,
}

/// Where a match and each group lie, as byte ranges of the text; a group
/// that did not take part is `None`.
pub type Region = Vec<Option<(usize, usize)>>;

/// An entry of the backtracking stack (`StackType`).
#[derive(Clone, Copy, Debug)]
enum Entry {
    Alt {
        pc: usize,
        pos: usize,
    },
    /// An entry an atomic group has cut (`STK_VOID`).
    Void,
    /// A capture's previous bounds, restored when popped.
    Mem {
        n: usize,
        start: Option<usize>,
        end: Option<usize>,
    },
    EmptyCheckStart {
        id: usize,
        pos: usize,
    },
    /// A repeat counter's previous value.
    Count {
        id: usize,
        prev: u32,
    },
    Mark {
        id: usize,
        pos: usize,
    },
    Keep {
        pos: usize,
    },
}

/// Where each required item (`Required`) next starts in one text, kept
/// across the searches of a global match so that each part of the text is
/// scanned once: a search at `s` looks for an item again only once `s`
/// has passed the place found before. A longest search reads on past its
/// own start, so an absence holds only from where it was found.
#[derive(Debug, Default)]
pub struct Hints {
    next: Vec<Next>,
}

/// Where a required item next starts.
#[derive(Clone, Copy, Debug)]
enum Next {
    /// Not looked for yet.
    Unknown,
    At(usize),
    /// Absent from this place on.
    Absent(usize),
}

/// Searches `text` from `start` (`onig_search` forward, range to the end).
pub fn search(
    prog: &Program,
    groups: usize,
    text: &[u8],
    start: usize,
    opts: SearchOptions,
    hints: &mut Hints,
) -> Result<Option<Region>, Error> {
    if start > text.len() {
        return Ok(None);
    }
    if hints.next.len() != prog.required.len() {
        hints.next = vec![Next::Unknown; prog.required.len()];
    }
    let mut m = Matcher {
        prog,
        text,
        search_start: start,
        opts,
        groups,
        best_len: None,
        best_region: None,
    };
    let mut s = start;
    loop {
        // A match starting past the last place a required item starts
        // cannot contain it.
        for (r, next) in prog.required.iter().zip(hints.next.iter_mut()) {
            *next = match *next {
                Next::At(p) if p >= s => Next::At(p),
                Next::Absent(from) if from <= s => Next::Absent(from),
                _ => r.first_in(text, s).map_or(Next::Absent(s), Next::At),
            };
            if matches!(next, Next::Absent(_)) {
                return Ok(m.best_region);
            }
        }
        if let Some(region) = m.match_at(s)?
            && !opts.longest
        {
            return Ok(Some(region));
        }
        if s >= text.len() {
            break;
        }
        s += enc::len(text, s);
    }
    Ok(m.best_region)
}

struct Matcher<'p, 't> {
    prog: &'p Program,
    text: &'t [u8],
    /// `msa->start`, where `\G` matches.
    search_start: usize,
    opts: SearchOptions,
    groups: usize,
    /// For the longest search: the best length over all starts so far
    /// (`msa->best_len`) and its region.
    best_len: Option<usize>,
    best_region: Option<Region>,
}

impl Matcher<'_, '_> {
    /// `match_at`: a match starting at `sstart`, or `None`.
    #[expect(
        clippy::too_many_lines,
        reason = "Oniguruma's one dispatch loop over the instructions; kept whole to stay comparable"
    )]
    fn match_at(&mut self, sstart: usize) -> Result<Option<Region>, Error> {
        let text = self.text;
        let end = text.len();
        let ops = &self.prog.ops;
        let mut stack: Vec<Entry> = Vec::new();
        let mut mbeg: Vec<Option<usize>> = vec![None; self.groups + 1];
        let mut mend: Vec<Option<usize>> = vec![None; self.groups + 1];
        let mut counts: Vec<u32> = vec![0; self.prog.repeats.len()];
        let mut keep = sstart;
        let mut local_best: Option<usize> = None;
        let mut retries: u64 = 0;
        let mut pc = 0;
        let mut s = sstart;
        // `fail:`: pop to the next alternative, undoing on the way.
        macro_rules! fail {
            () => {{
                loop {
                    match stack.pop() {
                        None => return Ok(None),
                        Some(Entry::Alt { pc: p, pos }) => {
                            retries += 1;
                            if retries > RETRY_LIMIT_IN_MATCH {
                                return Err(Error::new(E::RetryLimitInMatchOver));
                            }
                            pc = p;
                            s = pos;
                            break;
                        }
                        Some(Entry::Mem {
                            n,
                            start: a,
                            end: b,
                        }) => {
                            mbeg[n] = a;
                            mend[n] = b;
                        }
                        Some(Entry::Count { id, prev }) => counts[id] = prev,
                        Some(_) => {}
                    }
                }
                continue;
            }};
        }
        loop {
            match &ops[pc] {
                Op::End => {
                    let n = s - sstart;
                    if n == 0 && self.opts.not_empty {
                        fail!();
                    }
                    if self.opts.longest {
                        // A match counts only if no start so far has a
                        // longer one; either way, look on for longer. (The
                        // early stop for a match reaching the end needs
                        // the best to be this start's, which it then
                        // cannot exceed.)
                        if local_best.is_none_or(|b| n > b) && self.best_len.is_none_or(|b| n > b) {
                            local_best = Some(n);
                            self.best_len = Some(n);
                            let k = keep_of(&stack, keep).min(s);
                            self.best_region = Some(region(k, s, &mbeg, &mend));
                        }
                        fail!();
                    }
                    let k = keep.min(s);
                    return Ok(Some(region(k, s, &mbeg, &mend)));
                }
                Op::Str(b) => {
                    if end - s < b.len() || text[s..s + b.len()] != **b {
                        fail!();
                    }
                    s += b.len();
                }
                Op::Class(cc) => {
                    let Some(n) = class_step(cc, text, s) else {
                        fail!();
                    };
                    s = n;
                }
                Op::Any { multiline } => {
                    if s >= end {
                        fail!();
                    }
                    let n = enc::len(text, s);
                    if n < lead_len(text[s]) {
                        // `DATA_ENSURE(n)` on a character cut short.
                        fail!();
                    }
                    if !multiline && text[s] == b'\n' {
                        fail!();
                    }
                    s += n;
                }
                Op::Word { not } => {
                    if s >= end || unicode::is_word(enc::code(text, s)) == *not {
                        fail!();
                    }
                    s += enc::len(text, s);
                }
                Op::Anchor(a) => {
                    if !self.anchor(*a, s) {
                        fail!();
                    }
                }
                Op::Push(t) => stack.push(Entry::Alt { pc: *t, pos: s }),
                Op::Jump(t) => {
                    pc = *t;
                    continue;
                }
                Op::Pop => {
                    stack.pop();
                }
                Op::Fail => fail!(),
                Op::MemStart(n) => {
                    stack.push(Entry::Mem {
                        n: *n,
                        start: mbeg[*n],
                        end: mend[*n],
                    });
                    mbeg[*n] = Some(s);
                    mend[*n] = None;
                }
                Op::MemEnd(n) => {
                    stack.push(Entry::Mem {
                        n: *n,
                        start: mbeg[*n],
                        end: mend[*n],
                    });
                    mend[*n] = Some(s);
                }
                Op::EmptyCheckStart(id) => stack.push(Entry::EmptyCheckStart { id: *id, pos: s }),
                Op::EmptyCheckEnd(id) => {
                    let at = stack.iter().rev().find_map(|e| match e {
                        Entry::EmptyCheckStart { id: i, pos } if i == id => Some(*pos),
                        _ => None,
                    });
                    if at == Some(s) {
                        // An empty iteration: skip the loop's next step.
                        pc += 2;
                        continue;
                    }
                }
                Op::Repeat { id, greedy, after } => {
                    stack.push(Entry::Count {
                        id: *id,
                        prev: counts[*id],
                    });
                    counts[*id] = 0;
                    if self.prog.repeats[*id].0 == 0 {
                        if *greedy {
                            stack.push(Entry::Alt { pc: *after, pos: s });
                        } else {
                            stack.push(Entry::Alt { pc: pc + 1, pos: s });
                            pc = *after;
                            continue;
                        }
                    }
                }
                Op::RepeatInc { id, greedy, body } => {
                    stack.push(Entry::Count {
                        id: *id,
                        prev: counts[*id],
                    });
                    counts[*id] += 1;
                    let c = counts[*id];
                    let (lower, upper) = self.prog.repeats[*id];
                    if upper.is_some_and(|u| c >= u) {
                        // Done: go on after the loop.
                    } else if c >= lower {
                        if *greedy {
                            stack.push(Entry::Alt { pc: pc + 1, pos: s });
                            pc = *body;
                            continue;
                        }
                        stack.push(Entry::Alt { pc: *body, pos: s });
                    } else {
                        pc = *body;
                        continue;
                    }
                }
                Op::Mark(id) => stack.push(Entry::Mark { id: *id, pos: s }),
                Op::CutToMark { id, restore_pos } => {
                    // `STACK_TO_VOID_TO_MARK`: alternatives, empty-check
                    // starts and this mark are voided; captures, counters
                    // and saved values stay to be undone later.
                    for e in stack.iter_mut().rev() {
                        match *e {
                            Entry::Mark { id: i, pos } if i == *id => {
                                *e = Entry::Void;
                                if *restore_pos {
                                    s = pos;
                                }
                                break;
                            }
                            Entry::Alt { .. } | Entry::EmptyCheckStart { .. } => *e = Entry::Void,
                            _ => {}
                        }
                    }
                }
                Op::PopToMark(id) => {
                    // `STACK_POP_TIL_MARK`, undoing on the way.
                    while let Some(e) = stack.pop() {
                        match e {
                            Entry::Mark { id: i, .. } if i == *id => break,
                            Entry::Mem {
                                n,
                                start: a,
                                end: b,
                            } => {
                                mbeg[n] = a;
                                mend[n] = b;
                            }
                            Entry::Count { id, prev } => counts[id] = prev,
                            _ => {}
                        }
                    }
                }
                Op::StepBack(n) => {
                    let Some(p) = step_back(text, s, *n) else {
                        fail!();
                    };
                    s = p;
                }
                Op::Backref { groups, icase } => {
                    // `OP_BACKREF_N` for one group, `OP_BACKREF_MULTI` for
                    // several: the first group set that matches, with no
                    // going back to the others.
                    let mut ok = None;
                    for &g in groups {
                        let (Some(a), Some(b)) = (mbeg[g], mend[g]) else {
                            continue;
                        };
                        let n = b - a;
                        if n == 0 {
                            ok = Some(s);
                            break;
                        }
                        if end - s < n {
                            continue;
                        }
                        let got = if *icase {
                            cmp_ic(&text[a..b], text, s, s + n)
                        } else {
                            (text[a..b] == text[s..s + n]).then_some(s + n)
                        };
                        if got.is_some() {
                            ok = got;
                            break;
                        }
                    }
                    let Some(p) = ok else {
                        fail!();
                    };
                    s = p;
                }
                Op::BackrefCheck(groups) => {
                    if !groups
                        .iter()
                        .any(|&g| mbeg[g].is_some() && mend[g].is_some())
                    {
                        fail!();
                    }
                }
                Op::SaveKeep => stack.push(Entry::Keep { pos: s }),
                Op::UpdateKeep => keep = keep_of(&stack, keep),
            }
            pc += 1;
        }
    }
}

/// `STACK_GET_SAVE_VAL_TYPE_LAST(SAVE_KEEP)`: the last `\K` on the stack,
/// or `keep` as it was.
fn keep_of(stack: &[Entry], keep: usize) -> usize {
    stack
        .iter()
        .rev()
        .find_map(|e| match e {
            Entry::Keep { pos } => Some(*pos),
            _ => None,
        })
        .unwrap_or(keep)
}

/// The region of a match from `from` to `to`, its groups as the capture
/// arrays hold them.
fn region(from: usize, to: usize, mbeg: &[Option<usize>], mend: &[Option<usize>]) -> Region {
    let mut r = vec![Some((from, to))];
    for g in 1..mbeg.len() {
        r.push(match (mbeg[g], mend[g]) {
            (Some(a), Some(b)) => Some((a, b)),
            _ => None,
        });
    }
    r
}

impl Matcher<'_, '_> {
    fn anchor(&self, a: Anchor, s: usize) -> bool {
        let text = self.text;
        let end = text.len();
        let newline_at = |p: usize| p < end && text[p] == b'\n';
        match a {
            Anchor::BeginBuf => s == 0,
            Anchor::EndBuf => s == end,
            Anchor::BeginPosition => s == self.search_start,
            Anchor::BeginLine => {
                s == 0 || (s < end && enc::prev(text, 0, s).is_some_and(newline_at))
            }
            Anchor::EndLine => s == end || newline_at(s),
            Anchor::SemiEndBuf => s == end || (newline_at(s) && s + 1 == end),
            Anchor::WordBoundary | Anchor::NoWordBoundary => {
                let word_at = |p: usize| unicode::is_word(enc::code(text, p));
                let boundary = if s == 0 {
                    if a == Anchor::NoWordBoundary {
                        return !(s < end && word_at(s));
                    }
                    s < end && word_at(s)
                } else {
                    let prev = enc::prev(text, 0, s).expect("not at the start");
                    if s == end {
                        word_at(prev)
                    } else {
                        word_at(s) != word_at(prev)
                    }
                };
                boundary == (a == Anchor::WordBoundary)
            }
            Anchor::TextSegmentBoundary => egcb_break(text, s),
            Anchor::NoTextSegmentBoundary => !egcb_break(text, s),
        }
    }
}

/// The length of the character a lead byte starts, uncut.
fn lead_len(b: u8) -> usize {
    enc::len(&[b, 0x80, 0x80, 0x80], 0)
}

/// `onigenc_step_back`: `n` characters back from `s`.
fn step_back(text: &[u8], mut s: usize, n: u32) -> Option<usize> {
    for _ in 0..n {
        s = enc::prev(text, 0, s)?;
    }
    Some(s)
}

/// A class instruction at `s` (`OP_CCLASS*`): where the match goes on.
fn class_step(cc: &Class, text: &[u8], s: usize) -> Option<usize> {
    let end = text.len();
    if s >= end {
        return None;
    }
    let mb = lead_len(text[s]) != 1;
    let bit = cc.bit(u32::from(text[s]));
    let in_ranges = |s: usize| -> Option<bool> {
        let n = lead_len(text[s]);
        if end - s < n {
            return None;
        }
        Some(unicode::in_ranges(&cc.ranges, enc::code(text, s)))
    };
    // A multibyte character not in a negated class's ranges, or cut short.
    let mb_not = |s: usize| match in_ranges(s) {
        None => Some(end),
        Some(true) => None,
        Some(false) => Some(s + lead_len(text[s])),
    };
    match (cc.ranges.is_empty(), cc.bits == [0; 4], cc.not) {
        // `OP_CCLASS`
        (true, _, false) => (bit && !mb).then_some(s + 1),
        // `OP_CCLASS_NOT`
        (true, _, true) => (!bit).then(|| (s + lead_len(text[s])).min(end)),
        // `OP_CCLASS_MB`
        (false, true, false) => {
            if mb {
                in_ranges(s)?.then_some(s + lead_len(text[s]))
            } else {
                None
            }
        }
        // `OP_CCLASS_MB_NOT`
        (false, true, true) => {
            if mb {
                mb_not(s)
            } else {
                Some(s + 1)
            }
        }
        // `OP_CCLASS_MIX`
        (false, false, false) => {
            if mb {
                in_ranges(s)?.then_some(s + lead_len(text[s]))
            } else {
                bit.then_some(s + 1)
            }
        }
        // `OP_CCLASS_MIX_NOT`
        (false, false, true) => {
            if mb {
                mb_not(s)
            } else {
                (!bit).then_some(s + 1)
            }
        }
    }
}

/// `string_cmp_ic`: the captured bytes against the text from `s`, one
/// character's fold against the other's; the text read no further than
/// `limit`.
fn cmp_ic(cap: &[u8], text: &[u8], mut s: usize, limit: usize) -> Option<usize> {
    let mut p = 0;
    while p < cap.len() {
        let f1 = unicode::fold(enc::code(cap, p));
        p += enc::len(cap, p);
        if s >= limit {
            return None;
        }
        let f2 = unicode::fold(enc::code(&text[..limit], s));
        s += enc::len(&text[..limit], s);
        if f1 != f2 {
            return None;
        }
        if s >= limit && p < cap.len() {
            return None;
        }
    }
    Some(s)
}

/// `onigenc_egcb_is_break_position`.
fn egcb_break(text: &[u8], p: usize) -> bool {
    let end = text.len();
    if p == 0 || p == end {
        return true;
    }
    let Some(mut prev) = enc::prev(text, 0, p) else {
        return true;
    };
    let from = enc::code(text, prev);
    let to = enc::code(text, p);
    match egcb_2code(from, to) {
        Break::Not => false,
        Break::Yes => true,
        Break::UndefGb11 => {
            let pict = extended_pictographic();
            while let Some(q) = enc::prev(text, 0, prev) {
                prev = q;
                let c = enc::code(text, q);
                if unicode::is_ctype(c, pict) {
                    return false;
                }
                if unicode::egcb(c) != Some(Egcb::Extend) {
                    break;
                }
            }
            true
        }
        Break::UndefRiRi => {
            let mut n = 0;
            while let Some(q) = enc::prev(text, 0, prev) {
                prev = q;
                if unicode::egcb(enc::code(text, q)) != Some(Egcb::RegionalIndicator) {
                    break;
                }
                n += 1;
            }
            n % 2 != 0
        }
    }
}

fn extended_pictographic() -> usize {
    unicode::property(b"ExtendedPictographic").expect("a property Oniguruma has")
}

enum Break {
    Not,
    Yes,
    UndefGb11,
    UndefRiRi,
}

/// `unicode_egcb_is_break_2code`.
fn egcb_2code(from_c: u32, to_c: u32) -> Break {
    use Egcb as G;
    let from = unicode::egcb(from_c);
    let to = unicode::egcb(to_c);
    if from.is_none() && to.is_none() {
        return Break::Yes;
    }
    if from == Some(G::CR) && to == Some(G::LF) {
        return Break::Not;
    }
    let control = |t: Option<G>| matches!(t, Some(G::Control | G::CR | G::LF));
    if control(from) || control(to) {
        return Break::Yes;
    }
    let hangul = |t: Option<G>| matches!(t, Some(G::L | G::LV | G::LVT | G::T | G::V));
    if hangul(from) && hangul(to) {
        if from == Some(G::L) && to != Some(G::T) {
            return Break::Not;
        }
        if matches!(from, Some(G::LV | G::V)) && matches!(to, Some(G::V | G::T)) {
            return Break::Not;
        }
        if to == Some(G::T) && matches!(from, Some(G::LVT | G::T)) {
            return Break::Not;
        }
        return Break::Yes;
    }
    if matches!(to, Some(G::Extend | G::ZWJ | G::SpacingMark)) || from == Some(G::Prepend) {
        return Break::Not;
    }
    if from == Some(G::ZWJ) {
        if unicode::is_ctype(to_c, extended_pictographic()) {
            return Break::UndefGb11;
        }
        return Break::Yes;
    }
    if from == Some(G::RegionalIndicator) && to == Some(G::RegionalIndicator) {
        return Break::UndefRiRi;
    }
    Break::Yes
}
