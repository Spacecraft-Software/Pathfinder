// SPDX-FileCopyrightText: 2002-2024 K.Kosako
// SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
// SPDX-License-Identifier: BSD-2-Clause AND (GPL-3.0-or-later OR MIT)

//! The tree as a program for [`crate::exec`], in the shapes Oniguruma
//! compiles it to (`compile_tree` in `src/regcomp.c`): the shapes decide
//! where an empty iteration ends a loop and what an atomic group cuts, so
//! they are kept even where a simpler program would find a match too.

use crate::ast::{Anchor, Class, Node};
use crate::enc;

/// `QUANTIFIER_EXPAND_LIMIT_SIZE`: how many instructions a quantifier may
/// be unrolled into before it gets a counter.
const QUANTIFIER_EXPAND_LIMIT_SIZE: usize = 10;

/// An instruction (`OP_*`). Jump targets are absolute.
#[derive(Clone, Debug)]
pub enum Op {
    Str(Box<[u8]>),
    Class(Box<Class>),
    Any {
        multiline: bool,
    },
    Word {
        not: bool,
    },
    Anchor(Anchor),
    Push(usize),
    Jump(usize),
    /// Drops the top of the stack (`OP_POP`).
    Pop,
    Fail,
    MemStart(usize),
    MemEnd(usize),
    EmptyCheckStart(usize),
    /// Skips the next instruction when the body matched nothing.
    EmptyCheckEnd(usize),
    Repeat {
        id: usize,
        greedy: bool,
        /// Where the loop goes on after it.
        after: usize,
    },
    RepeatInc {
        id: usize,
        greedy: bool,
        /// Where the loop's body starts.
        body: usize,
    },
    /// Pushes a mark with the position (`OP_MARK`).
    Mark(usize),
    CutToMark {
        id: usize,
        restore_pos: bool,
    },
    /// Unwinds the stack, undoing captures, down to and with the mark.
    PopToMark(usize),
    /// Moves back this many characters, or fails (`OP_STEP_BACK_START`).
    StepBack(u32),
    Backref {
        groups: Box<[usize]>,
        icase: bool,
    },
    BackrefCheck(Box<[usize]>),
    SaveKeep,
    UpdateKeep,
    End,
}

/// A compiled pattern.
#[derive(Clone, Debug)]
pub struct Program {
    pub ops: Vec<Op>,
    /// Each repeat counter's bounds; `None` for no upper bound.
    pub repeats: Vec<(u32, Option<u32>)>,
    /// What every match must contain at or after its start (see
    /// [`Required`]).
    pub required: Vec<Required>,
}

/// Something every match contains at or after where it starts: an exact
/// string, or one of a set of ASCII bytes. Oniguruma's search finds such a
/// literal first (`set_optimize_info_from_tree`, `forward_search`) and
/// answers "no match" without matching when the text lacks it, which is
/// what keeps `(a*)*b` on a text with no `b` from reaching the retry
/// limit. Only what Oniguruma can take is taken: case-sensitive strings,
/// and classes or alternatives of single-byte characters (`[bc]`,
/// `b|c`); `\d`, which has multibyte members, gives nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Required {
    Exact(Box<[u8]>),
    Bytes([u64; 2]),
}

impl Required {
    /// The first place at or after `from` in `text` it starts at, if any.
    pub fn first_in(&self, text: &[u8], from: usize) -> Option<usize> {
        let t = &text[from..];
        match self {
            Self::Exact(b) => t.windows(b.len()).position(|w| w == &**b),
            Self::Bytes(set) => t
                .iter()
                .position(|&c| c < 0x80 && set[usize::from(c >> 6)] & (1 << (c & 63)) != 0),
        }
        .map(|p| p + from)
    }
}

/// The required items of `n` (see [`Required`]): those of every item of
/// a sequence, of a repeat's body when it repeats at least once, of a
/// positive lookahead (it reads on from the match), and for alternatives
/// the first item of each branch as a set of bytes.
fn required(n: &Node, out: &mut Vec<Required>) {
    match n {
        Node::Str {
            bytes,
            icase: false,
        } if !bytes.is_empty() => out.push(Required::Exact(bytes.clone().into_boxed_slice())),
        Node::Class(cc) => {
            if let Some(set) = ascii_set(cc) {
                out.push(Required::Bytes(set));
            }
        }
        Node::List(xs) => {
            for x in xs {
                required(x, out);
            }
        }
        Node::Quant { body, lower, .. } if *lower > 0 => required(body, out),
        Node::Capture { body, .. }
        | Node::Group(body)
        | Node::Atomic(body)
        | Node::Look {
            behind: false,
            not: false,
            body,
        } => required(body, out),
        Node::Alt(xs) => {
            let mut set = [0u64; 2];
            for x in xs {
                let mut items = Vec::new();
                required(x, &mut items);
                let Some(first) = items.first() else {
                    return;
                };
                match first {
                    Required::Exact(b) => {
                        let c = b[0];
                        if c >= 0x80 {
                            return;
                        }
                        set[usize::from(c >> 6)] |= 1 << (c & 63);
                    }
                    Required::Bytes(s) => {
                        set[0] |= s[0];
                        set[1] |= s[1];
                    }
                }
            }
            out.push(Required::Bytes(set));
        }
        _ => {}
    }
}

/// A class's members when they are all ASCII bytes and it is not negated.
fn ascii_set(cc: &Class) -> Option<[u64; 2]> {
    if cc.not || !cc.ranges.is_empty() || cc.bits[2] != 0 || cc.bits[3] != 0 {
        return None;
    }
    let set = [cc.bits[0], cc.bits[1]];
    (set != [0, 0]).then_some(set)
}

/// Compiles a tuned tree (`compile_tree`, then `OP_END`).
pub fn compile(root: &Node) -> Program {
    let mut c = Compiler {
        ops: Vec::new(),
        repeats: Vec::new(),
        empty_checks: 0,
        marks: 0,
        has_keep: false,
    };
    c.has_keep = has_keep(root);
    c.tree(root);
    if c.has_keep {
        c.ops.push(Op::UpdateKeep);
    }
    c.ops.push(Op::End);
    let mut req = Vec::new();
    required(root, &mut req);
    Program {
        ops: c.ops,
        repeats: c.repeats,
        required: req,
    }
}

fn has_keep(n: &Node) -> bool {
    match n {
        Node::Keep => true,
        Node::List(xs) | Node::Alt(xs) => xs.iter().any(has_keep),
        Node::Look { body, .. }
        | Node::LookBehind { body, .. }
        | Node::Quant { body, .. }
        | Node::Atomic(body)
        | Node::Capture { body, .. }
        | Node::Group(body) => has_keep(body),
        Node::IfElse { cond, then, els } => {
            has_keep(cond)
                || then.as_deref().is_some_and(has_keep)
                || els.as_deref().is_some_and(has_keep)
        }
        _ => false,
    }
}

struct Compiler {
    ops: Vec<Op>,
    repeats: Vec<(u32, Option<u32>)>,
    empty_checks: usize,
    marks: usize,
    has_keep: bool,
}

impl Compiler {
    fn here(&self) -> usize {
        self.ops.len()
    }

    fn mark(&mut self) -> usize {
        self.marks += 1;
        self.marks - 1
    }

    /// Emits a placeholder to patch once its target is known.
    fn hole(&mut self) -> usize {
        self.ops.push(Op::Fail);
        self.ops.len() - 1
    }

    #[expect(
        clippy::too_many_lines,
        reason = "Oniguruma's one switch over the node kinds; kept whole to stay comparable"
    )]
    fn tree(&mut self, n: &Node) {
        match n {
            Node::Empty => {}
            Node::Str { bytes, .. } => {
                if !bytes.is_empty() {
                    self.ops.push(Op::Str(bytes.clone().into_boxed_slice()));
                }
            }
            Node::Class(cc) => self.ops.push(Op::Class(cc.clone())),
            Node::Word { not } => self.ops.push(Op::Word { not: *not }),
            Node::Any { multiline } => self.ops.push(Op::Any {
                multiline: *multiline,
            }),
            Node::Anchor(a) => self.ops.push(Op::Anchor(*a)),
            Node::List(xs) => {
                for x in xs {
                    self.tree(x);
                }
            }
            Node::Alt(xs) => {
                let mut jumps = Vec::new();
                for (i, x) in xs.iter().enumerate() {
                    if i + 1 < xs.len() {
                        let push = self.hole();
                        self.tree(x);
                        jumps.push(self.hole());
                        self.ops[push] = Op::Push(self.here());
                    } else {
                        self.tree(x);
                    }
                }
                let end = self.here();
                for j in jumps {
                    self.ops[j] = Op::Jump(end);
                }
            }
            Node::Capture { num, body } => {
                self.ops.push(Op::MemStart(*num));
                self.tree(body);
                self.ops.push(Op::MemEnd(*num));
            }
            Node::Group(body) => self.tree(body),
            Node::Atomic(body) => {
                let id = self.mark();
                self.ops.push(Op::Mark(id));
                self.tree(body);
                self.ops.push(Op::CutToMark {
                    id,
                    restore_pos: false,
                });
            }
            Node::Look {
                behind: false,
                not: false,
                body,
            } => {
                let id = self.mark();
                self.ops.push(Op::Mark(id));
                self.tree(body);
                self.ops.push(Op::CutToMark {
                    id,
                    restore_pos: true,
                });
            }
            Node::Look {
                behind: false,
                not: true,
                body,
            } => {
                // `OP_PUSH`, `OP_MARK`, body, `OP_POP_TO_MARK`, `OP_POP`,
                // `OP_FAIL`.
                let id = self.mark();
                let push = self.hole();
                self.ops.push(Op::Mark(id));
                self.tree(body);
                self.ops.push(Op::PopToMark(id));
                self.ops.push(Op::Pop);
                self.ops.push(Op::Fail);
                self.ops[push] = Op::Push(self.here());
            }
            Node::Look { behind: true, .. } => {
                unreachable!("lookbehinds are tuned into `LookBehind`")
            }
            Node::LookBehind {
                not: false,
                chars,
                body,
            } => {
                // The position is not restored: the body, of a fixed
                // length, is taken to end where it started from, which a
                // start inside a character breaks, as in Oniguruma.
                let id = self.mark();
                self.ops.push(Op::Mark(id));
                self.ops.push(Op::StepBack(*chars));
                self.tree(body);
                self.ops.push(Op::CutToMark {
                    id,
                    restore_pos: false,
                });
            }
            Node::LookBehind {
                not: true,
                chars,
                body,
            } => {
                // `OP_MARK`, `OP_PUSH`, `OP_STEP_BACK_START`, body,
                // `OP_POP_TO_MARK`, `OP_FAIL`, then `OP_POP`.
                let id = self.mark();
                self.ops.push(Op::Mark(id));
                let push = self.hole();
                self.ops.push(Op::StepBack(*chars));
                self.tree(body);
                self.ops.push(Op::PopToMark(id));
                self.ops.push(Op::Fail);
                self.ops[push] = Op::Push(self.here());
                self.ops.push(Op::Pop);
            }
            Node::Quant {
                body,
                lower,
                upper,
                greedy,
            } => self.quant(body, *lower, *upper, *greedy),
            Node::Backref { groups, icase, .. } => self.ops.push(Op::Backref {
                groups: groups.clone().into_boxed_slice(),
                icase: *icase,
            }),
            Node::BackrefCheck { groups } => {
                self.ops
                    .push(Op::BackrefCheck(groups.clone().into_boxed_slice()));
            }
            Node::IfElse { cond, then, els } => {
                // `OP_MARK`, `OP_PUSH` else, cond, `OP_CUT_TO_MARK`, then,
                // `OP_JUMP` end, else: `OP_CUT_TO_MARK`, else.
                let id = self.mark();
                self.ops.push(Op::Mark(id));
                let push = self.hole();
                self.tree(cond);
                self.ops.push(Op::CutToMark {
                    id,
                    restore_pos: false,
                });
                if let Some(t) = then {
                    self.tree(t);
                }
                let jump = self.hole();
                self.ops[push] = Op::Push(self.here());
                self.ops.push(Op::CutToMark {
                    id,
                    restore_pos: false,
                });
                if let Some(e) = els {
                    self.tree(e);
                }
                self.ops[jump] = Op::Jump(self.here());
            }
            Node::Keep => self.ops.push(Op::SaveKeep),
            Node::Fail => self.ops.push(Op::Fail),
        }
    }

    /// `compile_quantifier_node`.
    fn quant(&mut self, body: &Node, lower: u32, upper: Option<u32>, greedy: bool) {
        let tlen = length(body);
        if tlen == 0 {
            return;
        }
        let empty = min_byte_len(body) == 0;
        let lower_n = lower as usize;
        let infinite = upper.is_none();
        if infinite && (lower <= 1 || tlen.saturating_mul(lower_n) <= QUANTIFIER_EXPAND_LIMIT_SIZE)
        {
            // A long body repeated at least once is entered by a jump past
            // the loop's alternative rather than compiled twice.
            let into = if lower == 1 && tlen > QUANTIFIER_EXPAND_LIMIT_SIZE {
                Some(self.hole())
            } else {
                for _ in 0..lower {
                    self.tree(body);
                }
                None
            };
            if greedy {
                // L: `OP_PUSH` out, body, `OP_JUMP` L.
                let top = self.here();
                let push = self.hole();
                let start = self.here();
                self.body_with_check(body, empty);
                self.ops.push(Op::Jump(top));
                self.ops[push] = Op::Push(self.here());
                if let Some(j) = into {
                    self.ops[j] = Op::Jump(start);
                }
            } else {
                // `OP_JUMP` P, L: body, P: `OP_PUSH` L.
                let jump = self.hole();
                let start = self.here();
                self.body_with_check(body, empty);
                self.ops[jump] = Op::Jump(self.here());
                self.ops.push(Op::Push(start));
                if let Some(j) = into {
                    self.ops[j] = Op::Jump(start);
                }
            }
            return;
        }
        if upper == Some(0) {
            return;
        }
        let upper_n = upper.unwrap_or(0) as usize;
        if !infinite
            && greedy
            && (upper_n == 1 || (tlen + 1).saturating_mul(upper_n) <= QUANTIFIER_EXPAND_LIMIT_SIZE)
        {
            for _ in 0..lower {
                self.tree(body);
            }
            let mut pushes = Vec::new();
            for _ in lower..upper.unwrap_or(0) {
                pushes.push(self.hole());
                self.tree(body);
            }
            let end = self.here();
            for p in pushes {
                self.ops[p] = Op::Push(end);
            }
            return;
        }
        if !greedy && upper == Some(1) && lower == 0 {
            // `??`: `OP_PUSH` body, `OP_JUMP` over it.
            let push = self.hole();
            let jump = self.hole();
            self.ops[push] = Op::Push(self.here());
            self.tree(body);
            self.ops[jump] = Op::Jump(self.here());
            return;
        }
        // `compile_range_repeat_node`.
        let id = self.repeats.len();
        self.repeats.push((lower, upper));
        let rep = self.hole();
        let start = self.here();
        self.body_with_check(body, empty);
        self.ops.push(Op::RepeatInc {
            id,
            greedy,
            body: start,
        });
        self.ops[rep] = Op::Repeat {
            id,
            greedy,
            after: self.here(),
        };
    }

    /// `compile_quant_body_with_empty_check`.
    fn body_with_check(&mut self, body: &Node, empty: bool) {
        if empty {
            let id = self.empty_checks;
            self.empty_checks += 1;
            self.ops.push(Op::EmptyCheckStart(id));
            self.tree(body);
            self.ops.push(Op::EmptyCheckEnd(id));
        } else {
            self.tree(body);
        }
    }
}

/// How many instructions `n` compiles to (`compile_length_tree`), which
/// decides how a quantifier over it is compiled.
fn length(n: &Node) -> usize {
    match n {
        Node::Empty => 0,
        Node::Str { bytes, .. } => string_ops(bytes),
        Node::Class(_)
        | Node::Word { .. }
        | Node::Any { .. }
        | Node::Anchor(_)
        | Node::Backref { .. }
        | Node::BackrefCheck { .. }
        | Node::Keep
        | Node::Fail => 1,
        Node::List(xs) => xs.iter().map(length).sum(),
        Node::Alt(xs) => {
            let n = xs.len();
            xs.iter().map(length).sum::<usize>() + 2 * n.saturating_sub(1)
        }
        Node::Group(body) => length(body),
        Node::Capture { body, .. }
        | Node::Atomic(body)
        | Node::Look {
            not: false, body, ..
        } => length(body) + 2,
        Node::Look {
            not: true, body, ..
        } => length(body) + 5,
        Node::LookBehind {
            not: false, body, ..
        } => length(body) + 3,
        Node::LookBehind {
            not: true, body, ..
        } => length(body) + 6,
        Node::IfElse { cond, then, els } => {
            length(cond) + then.as_deref().map_or(0, length) + els.as_deref().map_or(0, length) + 5
        }
        Node::Quant {
            body,
            lower,
            upper,
            greedy,
        } => quant_length(body, *lower, *upper, *greedy),
    }
}

/// `compile_length_quantifier_node`.
fn quant_length(body: &Node, lower: u32, upper: Option<u32>, greedy: bool) -> usize {
    let tlen = length(body);
    if tlen == 0 {
        return 0;
    }
    let mod_tlen = if min_byte_len(body) == 0 {
        tlen + 2
    } else {
        tlen
    };
    let lower_n = lower as usize;
    match upper {
        None if lower <= 1 || tlen.saturating_mul(lower_n) <= QUANTIFIER_EXPAND_LIMIT_SIZE => {
            let head = if lower == 1 && tlen > QUANTIFIER_EXPAND_LIMIT_SIZE {
                1
            } else {
                tlen * lower_n
            };
            head + 1 + mod_tlen + 1
        }
        Some(0) => 0,
        Some(u)
            if greedy
                && (u == 1
                    || (tlen + 1).saturating_mul(u as usize) <= QUANTIFIER_EXPAND_LIMIT_SIZE) =>
        {
            tlen * lower_n + (1 + tlen) * (u as usize - lower_n)
        }
        Some(1) if !greedy && lower == 0 => 2 + tlen,
        _ => mod_tlen + 2,
    }
}

/// The instructions a string compiles to: one per run of characters of
/// one byte length (`compile_length_string_node`).
fn string_ops(b: &[u8]) -> usize {
    let mut n = 0;
    let mut prev = 0;
    let mut i = 0;
    while i < b.len() {
        let l = enc::len(b, i);
        if l != prev {
            n += 1;
            prev = l;
        }
        i += l;
    }
    n
}

/// The fewest bytes `n` can match (`node_min_byte_len`).
pub fn min_byte_len(n: &Node) -> usize {
    match n {
        Node::Str { bytes, .. } => bytes.len(),
        Node::Class(_) | Node::Word { .. } | Node::Any { .. } => 1,
        Node::List(xs) => xs.iter().map(min_byte_len).sum(),
        Node::Alt(xs) => xs.iter().map(min_byte_len).min().unwrap_or(0),
        Node::Capture { body, .. } | Node::Group(body) | Node::Atomic(body) => min_byte_len(body),
        Node::Quant { body, lower, .. } => min_byte_len(body).saturating_mul(*lower as usize),
        Node::IfElse { cond, then, els } => {
            let a = min_byte_len(cond) + then.as_deref().map_or(0, min_byte_len);
            let b = els.as_deref().map_or(0, min_byte_len);
            a.min(b)
        }
        // A backreference's group can match nothing.
        _ => 0,
    }
}
