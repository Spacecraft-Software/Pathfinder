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
