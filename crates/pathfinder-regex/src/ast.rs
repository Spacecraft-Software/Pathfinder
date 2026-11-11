// SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
// SPDX-License-Identifier: GPL-3.0-or-later OR MIT

//! The parsed pattern: Oniguruma's node kinds (`regparse.h`), with the
//! options that were on where each node was written already applied.

/// A character class (`CClassNode`): a bitset for the codes Oniguruma keeps
/// as single bytes, sorted ranges for the rest, and the negation.
///
/// Which part a code lives in follows Oniguruma: in UTF-8 a code below
/// 0x80, or a crude byte (`\xHH`, an octal escape), is a single byte; any
/// other code point is multibyte, and a range written `[a-é]` lands in
/// both. How a character is looked up depends on which parts the class
/// has (`OP_CCLASS*`, in [`crate::exec`]). `Perl_NG` has no class set
/// operations, so a class is never combined with another.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Class {
    pub bits: [u64; 4],
    pub ranges: Vec<(u32, u32)>,
    pub not: bool,
}

impl Class {
    pub fn set_bit(&mut self, b: u32) {
        self.bits[(b >> 6) as usize] |= 1 << (b & 63);
    }

    pub fn bit(&self, b: u32) -> bool {
        b < 256 && self.bits[(b >> 6) as usize] & (1 << (b & 63)) != 0
    }

    pub fn set_bits(&mut self, from: u32, to: u32) {
        for b in from..=to.min(0xff) {
            self.set_bit(b);
        }
    }

    /// Adds `from..=to` to the ranges, merging (`add_code_range_to_buf`).
    pub fn add_range(&mut self, from: u32, to: u32) {
        let (from, to) = if from <= to { (from, to) } else { (to, from) };
        // Ranges mostly come in ascending order (a ctype's table): one that
        // starts at or past the last range's start is appended or merged
        // into it, keeping a class of `\w`'s ranges linear to build.
        match self.ranges.last_mut() {
            None => {
                self.ranges.push((from, to));
                return;
            }
            Some(last) if last.1.saturating_add(1) < from => {
                self.ranges.push((from, to));
                return;
            }
            Some(last) if last.0 <= from => {
                last.1 = last.1.max(to);
                return;
            }
            Some(_) => {}
        }
        let mut lo = from;
        let mut hi = to;
        let mut out = Vec::with_capacity(self.ranges.len() + 1);
        let mut placed = false;
        for &(a, b) in &self.ranges {
            if b.saturating_add(1) < lo {
                out.push((a, b));
            } else if hi.saturating_add(1) < a {
                if !placed {
                    out.push((lo, hi));
                    placed = true;
                }
                out.push((a, b));
            } else {
                lo = lo.min(a);
                hi = hi.max(b);
            }
        }
        if !placed {
            out.push((lo, hi));
        }
        self.ranges = out;
    }

    /// Adds ranges sorted by their start, as many `add_range` calls would,
    /// in one merge: a class given `\w` a thousand times (`[\w\w…`) stays
    /// linear in the ranges.
    pub fn add_sorted_ranges(&mut self, new: &[(u32, u32)]) {
        let after = |l: &(u32, u32)| new.first().is_none_or(|n| l.0 <= n.0);
        if self.ranges.last().is_none_or(after) {
            for &(from, to) in new {
                self.add_range(from, to);
            }
            return;
        }
        let old = core::mem::take(&mut self.ranges);
        let mut out: Vec<(u32, u32)> = Vec::with_capacity(old.len() + new.len());
        let (mut i, mut j) = (0, 0);
        while i < old.len() || j < new.len() {
            let next = if j >= new.len() || (i < old.len() && old[i].0 <= new[j].0) {
                i += 1;
                old[i - 1]
            } else {
                j += 1;
                new[j - 1]
            };
            match out.last_mut() {
                Some(last) if next.0 <= last.1.saturating_add(1) => last.1 = last.1.max(next.1),
                _ => out.push(next),
            }
        }
        self.ranges = out;
    }

    /// Membership before the negation.
    pub fn holds(&self, code: u32, len: usize) -> bool {
        if len > 1 || code >= 256 {
            crate::unicode::in_ranges(&self.ranges, code)
        } else {
            self.bit(code)
        }
    }

    /// Membership before the negation of a code point written as UTF-8
    /// (`onig_is_code_in_cc`).
    pub fn holds_code(&self, code: u32) -> bool {
        self.holds(code, crate::enc::code_len(code).unwrap_or(1))
    }

    /// Adds a code point where its UTF-8 length puts it
    /// (`ADD_CODE_INTO_CC`).
    pub fn add_code(&mut self, code: u32) {
        if crate::enc::code_len(code) == Some(1) {
            self.set_bit(code);
        } else {
            self.add_range(code, code);
        }
    }
}

/// Anchors (`ANCR_*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor {
    BeginBuf,
    BeginLine,
    BeginPosition,
    EndBuf,
    SemiEndBuf,
    EndLine,
    WordBoundary,
    NoWordBoundary,
    TextSegmentBoundary,
    NoTextSegmentBoundary,
}

/// A node of the pattern.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Node {
    Empty,
    /// Literal bytes, case-insensitive when `icase`.
    Str {
        bytes: Vec<u8>,
        icase: bool,
    },
    Class(Box<Class>),
    /// `\w`, `\W` and `\p{Word}` (a ctype node, not a class).
    Word {
        not: bool,
    },
    /// `.`; `multiline` lets it match a newline.
    Any {
        multiline: bool,
    },
    Anchor(Anchor),
    /// Lookahead and lookbehind, positive or negative, as parsed.
    Look {
        behind: bool,
        not: bool,
        body: Box<Node>,
    },
    /// A lookbehind once tuned: its body matches exactly `chars`
    /// characters, ending where the lookbehind stands.
    LookBehind {
        not: bool,
        chars: u32,
        body: Box<Node>,
    },
    List(Vec<Node>),
    Alt(Vec<Node>),
    Quant {
        body: Box<Node>,
        lower: u32,
        /// `None` for no upper bound.
        upper: Option<u32>,
        greedy: bool,
    },
    /// `(?>…)`, and a possessive quantifier.
    Atomic(Box<Node>),
    /// A capture group, numbered from 1.
    Capture {
        num: usize,
        body: Box<Node>,
    },
    /// An option group `(?i:…)` (`BAG_OPTION`), and a group at the head of
    /// an if-else branch (a list); a plain `(?:…)` is its body.
    Group(Box<Node>),
    /// `\1`, `\k<name>`: the first of the groups that is set and matches;
    /// `icase` compares case-insensitively, `level` is `\k<n+level>`;
    /// `recursive` when written inside a group it names
    /// (`ND_IS_RECURSION`).
    Backref {
        groups: Vec<usize>,
        icase: bool,
        level: Option<i32>,
        recursive: bool,
    },
    /// `(?(n)…)`'s condition: whether any of the groups has matched.
    BackrefCheck {
        groups: Vec<usize>,
    },
    /// `(?(cond)then|else)`; the condition is atomic.
    IfElse {
        cond: Box<Node>,
        then: Option<Box<Node>>,
        els: Option<Box<Node>>,
    },
    /// `\K`.
    Keep,
    /// A node that never matches (`onig_node_reset_fail`, a gimmick): what
    /// a negative lookbehind of nothing becomes.
    Fail,
}

#[cfg(test)]
mod tests {
    use super::Class;

    /// `add_sorted_ranges` is `add_range` over each range, whatever the
    /// class held before: before, after, around or across the new ones.
    #[test]
    fn sorted_ranges_merge_as_add_range_does() {
        let new = [
            (0x100, 0x120),
            (0x122, 0x130),
            (0x400, 0x500),
            (0x501, 0x501),
        ];
        let olds: [&[(u32, u32)]; 5] = [
            &[],
            &[(0x80, 0x90)],
            &[(0x600, 0x700)],
            &[(0x110, 0x121), (0x131, 0x3ff), (0x1_0000, 0x10_ffff)],
            &[(0x80, 0x10_ffff)],
        ];
        for old in olds {
            let mut one = Class::default();
            let mut each = Class::default();
            for &(a, b) in old {
                one.add_range(a, b);
                each.add_range(a, b);
            }
            one.add_sorted_ranges(&new);
            for &(a, b) in &new {
                each.add_range(a, b);
            }
            assert_eq!(one.ranges, each.ranges, "over {old:?}");
        }
    }
}
