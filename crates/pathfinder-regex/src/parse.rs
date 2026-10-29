// SPDX-FileCopyrightText: 2002-2024 K.Kosako
// SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
// SPDX-License-Identifier: BSD-2-Clause AND (GPL-3.0-or-later OR MIT)

//! Oniguruma's parser (`src/regparse.c`), transcribed for the one syntax
//! and encoding jq compiles with: `ONIG_SYNTAX_PERL_NG` and UTF-8.
//!
//! The functions keep Oniguruma's names (`fetch_token`, `prs_cc`,
//! `prs_bag`, `prs_exp`, …) and their control flow, with the branches the
//! syntax turns off removed, so that a pattern is accepted, rejected and
//! read exactly as jq reads it, bugs included: a group name is checked
//! only at its first character, an octal escape past 0377 is a byte of its
//! low eight bits, and an interval Oniguruma cannot read is literal text.

use crate::ast::{Anchor, Class, Node};
use crate::enc;
use crate::error::{Error, ErrorKind as E};
use crate::unicode::{self, ctype};

/// `ONIG_MAX_REPEAT_NUM`.
const MAX_REPEAT_NUM: u32 = 100_000;
/// `ONIG_MAX_BACKREF_NUM`.
const MAX_BACKREF_NUM: u32 = 1000;
/// `DEFAULT_PARSE_DEPTH_LIMIT`.
const PARSE_DEPTH_LIMIT: u32 = 4096;

/// The options that shape parsing (`ONIG_OPTION_*`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Options {
    pub ignorecase: bool,
    pub extend: bool,
    /// `.` matches a newline (Ruby's "multiline").
    pub multiline: bool,
    /// `^` is `\A`, `$` is `\Z`.
    pub singleline: bool,
}

/// What a pattern parses to.
#[derive(Debug)]
pub struct Parsed {
    pub root: Node,
    /// The number of capture groups.
    pub groups: usize,
    /// Each group name and the groups it names, in definition order.
    pub names: Vec<(Vec<u8>, Vec<usize>)>,
}

/// Parses `pattern` (`onig_parse_tree`).
pub fn parse(pattern: &[u8], options: Options) -> Result<Parsed, Error> {
    if !enc::is_valid(pattern) {
        return Err(Error::new(E::InvalidCodePointValue));
    }
    let mut p = Parser {
        s: pattern,
        p: 0,
        prev: 0,
        opts: options,
        groups: 0,
        group_nodes: Vec::new(),
        names: Vec::new(),
        depth: 0,
        tok: Tok::Eot,
        backp: 0,
        escaped: false,
        base: 0,
        cp_continue: false,
    };
    let mut t = p.fetch_token()?;
    let root = p.prs_alts(&mut t, Term::Eot, false)?.0;
    let mut root = root;
    crate::tune::tune(&mut root, p.groups)?;
    Ok(Parsed {
        root,
        groups: p.groups,
        names: p.names,
    })
}

/// A repeat operator (`tok->u.repeat`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rep {
    lower: u32,
    upper: Option<u32>,
    greedy: bool,
    possessive: bool,
    interval: bool,
}

/// The tokens outside a class (`TK_*`).
#[derive(Clone, Debug, PartialEq, Eq)]
enum Tok {
    Eot,
    /// One character, from `backp` to the parser's position.
    Str,
    CrudeByte(u8),
    CodePoint(u32),
    Repeat(Rep),
    Alt,
    Open,
    Close,
    CharType {
        ctype: usize,
        not: bool,
    },
    Prop {
        not: bool,
    },
    Anchor(Anchor),
    Keep,
    GeneralNewline,
    NoNewline,
    TrueAnychar,
    TextSegment,
    Backref {
        groups: Vec<usize>,
        level: Option<i32>,
    },
    Call,
    QuoteOpen,
    OpenCc,
    AnyChar,
}

/// The tokens inside a class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CcTok {
    Eot,
    Char { code: u32, escaped: bool },
    CrudeByte(u8),
    CodePoint(u32),
    Close,
    Range,
    CharType { ctype: usize, not: bool },
    Prop { not: bool },
    PosixOpen,
}

/// Where a token ends a run of alternatives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Term {
    Eot,
    Close,
}

/// A class's parse state (`CSTATE`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CState {
    Value,
    Range,
    Complete,
    Start,
}

/// The kind of the class value pending (`CVAL`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CVal {
    Sb,
    Mb,
    CProp,
    Undef,
}

/// `fetch_name`'s reading of a name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RefNum {
    No,
    Abs,
    Rel,
}

struct Parser<'a> {
    s: &'a [u8],
    p: usize,
    /// The position before the last `fetch`, for `unfetch` (`PUNFETCH`).
    prev: usize,
    opts: Options,
    groups: usize,
    /// For each group, whether its closing parenthesis has been read.
    group_nodes: Vec<bool>,
    names: Vec<(Vec<u8>, Vec<usize>)>,
    depth: u32,
    /// The token read last, its start and properties (`PToken`).
    tok: Tok,
    backp: usize,
    escaped: bool,
    base: u32,
    cp_continue: bool,
}

fn is_digit(c: u32) -> bool {
    (u32::from(b'0')..=u32::from(b'9')).contains(&c)
}

fn is_xdigit(c: u32) -> bool {
    is_digit(c) || (0x41..=0x46).contains(&c) || (0x61..=0x66).contains(&c)
}

fn xdigit_val(c: u32) -> u32 {
    match c {
        0x30..=0x39 => c - 0x30,
        0x41..=0x46 => c - 0x41 + 10,
        _ => c - 0x61 + 10,
    }
}

const fn ch(c: u8) -> u32 {
    c as u32
}

impl Parser<'_> {
    fn end(&self) -> bool {
        self.p >= self.s.len()
    }

    /// `PFETCH`.
    fn fetch(&mut self) -> u32 {
        let c = enc::code(self.s, self.p);
        self.prev = self.p;
        self.p += enc::len(self.s, self.p);
        c
    }

    /// `PUNFETCH`.
    fn unfetch(&mut self) {
        self.p = self.prev;
    }

    /// `PPEEK`: 0 at the end.
    fn peek(&self) -> u32 {
        if self.end() {
            0
        } else {
            enc::code(self.s, self.p)
        }
    }

    fn peek_is(&self, c: u8) -> bool {
        !self.end() && self.peek() == ch(c)
    }

    /// `PINC`.
    fn inc(&mut self) {
        self.prev = self.p;
        self.p += enc::len(self.s, self.p);
    }

    fn deeper(&mut self) -> Result<(), Error> {
        self.depth += 1;
        if self.depth > PARSE_DEPTH_LIMIT {
            return Err(Error::new(E::ParseDepthLimitOver));
        }
        Ok(())
    }

    fn named(&self, kind: E, from: usize, to: usize) -> Error {
        Error::with_name(kind, &self.s[from..to.min(self.s.len())])
    }

    // --- numbers -----------------------------------------------------

    /// `scan_number`: decimal digits, `None` on overflow.
    fn scan_number(&mut self) -> Option<u32> {
        let mut num: u32 = 0;
        let mut over = false;
        while !self.end() {
            let c = self.fetch();
            if is_digit(c) {
                let v = c - 0x30;
                if over || (i32::MAX as u32 - v) / 10 < num {
                    over = true;
                } else {
                    num = num * 10 + v;
                }
            } else {
                self.unfetch();
                break;
            }
        }
        (!over).then_some(num)
    }

    /// `scan_number` over `self.s[from..to]` without moving.
    fn number_in(&self, from: usize, to: usize) -> Option<u32> {
        let mut num: u32 = 0;
        for &b in &self.s[from..to] {
            if !b.is_ascii_digit() {
                break;
            }
            let v = u32::from(b - b'0');
            if (i32::MAX as u32 - v) / 10 < num {
                return None;
            }
            num = num * 10 + v;
        }
        Some(num)
    }

    /// `scan_hexadecimal_number`.
    fn scan_hex(&mut self, min: usize, max: usize) -> Result<u32, Error> {
        let mut code: u32 = 0;
        let mut n = 0;
        while !self.end() && n < max {
            let c = self.fetch();
            if is_xdigit(c) {
                n += 1;
                let v = xdigit_val(c);
                if (u32::MAX - v) / 16 < code {
                    return Err(Error::new(E::TooBigNumber));
                }
                code = (code << 4) + v;
            } else {
                self.unfetch();
                break;
            }
        }
        if n < min {
            return Err(Error::new(E::InvalidCodePointValue));
        }
        Ok(code)
    }

    /// `scan_octal_number`.
    fn scan_octal(&mut self, min: usize, max: usize) -> Result<u32, Error> {
        let mut code: u32 = 0;
        let mut n = 0;
        while !self.end() && n < max {
            let c = self.fetch();
            if is_digit(c) && c < ch(b'8') {
                n += 1;
                let v = c - 0x30;
                if (u32::MAX - v) / 8 < code {
                    return Err(Error::new(E::TooBigNumber));
                }
                code = (code << 3) + v;
            } else {
                self.unfetch();
                break;
            }
        }
        if n < min {
            return Err(Error::new(E::InvalidCodePointValue));
        }
        Ok(code)
    }

    /// `scan_number_of_base`.
    fn scan_base(&mut self, min: usize, base: u32) -> Result<u32, Error> {
        if base == 16 {
            self.scan_hex(min, 8)
        } else {
            self.scan_octal(min, 11)
        }
    }

    // --- interval and escapes ---------------------------------------

    /// `fetch_interval`. `Ok(None)` is "not an interval: a literal `{`";
    /// `Ok(Some((rep, fixed)))` an interval, `fixed` for `{n}`.
    fn fetch_interval(&mut self) -> Result<Option<(Rep, bool)>, Error> {
        let start = self.p;
        if self.end() {
            return Ok(None);
        }
        let low = match self.scan_number() {
            Some(n) if n <= MAX_REPEAT_NUM => n,
            _ => return Err(Error::new(E::TooBigNumberForRepeatRange)),
        };
        if self.p == start {
            // No `ONIG_SYN_ALLOW_INTERVAL_LOW_ABBREV`: `{,n}` is literal.
            self.p = start;
            return Ok(None);
        }
        let invalid = |me: &mut Self| {
            me.p = start;
            Ok(None)
        };
        if self.end() {
            return invalid(self);
        }
        let mut fixed = false;
        let c = self.fetch();
        let up;
        if c == ch(b',') {
            let prev = self.p;
            let n = match self.scan_number() {
                Some(n) if n <= MAX_REPEAT_NUM => n,
                _ => return Err(Error::new(E::TooBigNumberForRepeatRange)),
            };
            up = if self.p == prev { None } else { Some(n) };
        } else {
            self.unfetch();
            up = Some(low);
            fixed = true;
        }
        if self.end() {
            return invalid(self);
        }
        let c = self.fetch();
        if c != ch(b'}') {
            return invalid(self);
        }
        if let Some(up) = up
            && low > up
        {
            // `ONIG_SYN_OP2_PLUS_POSSESSIVE_INTERVAL`.
            return Err(Error::new(E::UpperSmallerThanLowerInRepeatRange));
        }
        Ok(Some((
            Rep {
                lower: low,
                upper: up,
                greedy: true,
                possessive: false,
                interval: true,
            },
            fixed,
        )))
    }

    /// `fetch_escaped_value_raw`, after the backslash.
    fn fetch_escaped_value(&mut self) -> Result<u32, Error> {
        if self.end() {
            return Err(Error::new(E::EndPatternAtEscape));
        }
        let c = self.fetch();
        let v = if c == ch(b'c') {
            // `ONIG_SYN_OP_ESC_C_CONTROL`.
            if self.end() {
                return Err(Error::new(E::EndPatternAtControl));
            }
            let c = self.fetch();
            if c == ch(b'?') {
                0o177
            } else {
                let c = if c == ch(b'\\') {
                    self.fetch_escaped_value()?
                } else {
                    c
                };
                c & 0x9f
            }
        } else {
            conv_backslash_value(c)
        };
        if enc::code_len(v).is_none() {
            return Err(Error::new(E::InvalidCodePointValue));
        }
        Ok(v)
    }

    /// The `brace_code_point_entry` of `\x{…}` and `\o{…}`: a code point,
    /// or the start of a sequence of them. `Ok(None)` when nothing was
    /// read: the escape is then the letter itself.
    fn brace_code_point(
        &mut self,
        prev: usize,
        base: u32,
        in_cc: Option<CState>,
    ) -> Result<Option<u32>, Error> {
        self.inc();
        let code = if base == 16 {
            let code = self.scan_hex(0, 8)?;
            if !self.end() && is_xdigit(self.peek()) {
                return Err(Error::new(E::TooLongWideCharValue));
            }
            code
        } else {
            let code = self.scan_octal(0, 11)?;
            if !self.end() && is_digit(self.peek()) {
                return Err(Error::new(E::TooLongWideCharValue));
            }
            code
        };
        self.base = base;
        if self.p > prev + enc::len(self.s, prev) {
            if self.end() {
                return Err(Error::new(E::InvalidCodePointValue));
            }
            if self.peek_is(b'}') {
                self.inc();
            } else {
                let n = match in_cc {
                    None => self.check_code_point_sequence(base)?,
                    Some(state) => self.check_code_point_sequence_cc(base, state)?,
                };
                if n == 0 {
                    return Err(Error::new(E::InvalidCodePointValue));
                }
                self.cp_continue = true;
            }
            Ok(Some(code))
        } else {
            self.p = prev;
            Ok(None)
        }
    }

    /// `check_code_point_sequence`: how many more code points follow,
    /// without moving.
    fn check_code_point_sequence(&mut self, base: u32) -> Result<usize, Error> {
        let save = (self.p, self.prev);
        let r = self.code_point_sequence(base, None);
        (self.p, self.prev) = save;
        r
    }

    fn check_code_point_sequence_cc(&mut self, base: u32, state: CState) -> Result<usize, Error> {
        let save = (self.p, self.prev);
        let start = if state == CState::Range {
            Cps::Empty
        } else {
            Cps::Start
        };
        let r = self.code_point_sequence(base, Some(start));
        (self.p, self.prev) = save;
        r
    }

    /// The scan behind both checks; `cc` is the class state, `None`
    /// outside a class.
    fn code_point_sequence(&mut self, base: u32, mut cc: Option<Cps>) -> Result<usize, Error> {
        let divide = |c: u32| c == ch(b' ') || c == ch(b'\n');
        let mut end_digit = false;
        let mut n = 0;
        while !self.end() {
            let mut c = self.fetch();
            if c == ch(b'}') {
                if cc == Some(Cps::Range) {
                    return Err(Error::new(E::InvalidCodePointValue));
                }
                return Ok(n);
            }
            if divide(c) {
                while !self.end() {
                    c = self.fetch();
                    if !divide(c) {
                        break;
                    }
                }
                if divide(c) {
                    return Err(Error::new(E::InvalidCodePointValue));
                }
            } else if cc.is_some() && c == ch(b'-') {
                if cc != Some(Cps::Start) || self.end() {
                    return Err(Error::new(E::InvalidCodePointValue));
                }
                end_digit = false;
                cc = Some(Cps::Range);
                continue;
            } else if end_digit {
                let more = if base == 16 {
                    is_xdigit(c)
                } else {
                    is_digit(c) && c < ch(b'8')
                };
                return Err(Error::new(if more {
                    E::TooLongWideCharValue
                } else {
                    E::InvalidCodePointValue
                }));
            }
            if c == ch(b'}') {
                if cc == Some(Cps::Range) {
                    return Err(Error::new(E::InvalidCodePointValue));
                }
                return Ok(n);
            }
            if cc.is_some() && c == ch(b'-') {
                if cc != Some(Cps::Start) || self.end() {
                    return Err(Error::new(E::InvalidCodePointValue));
                }
                end_digit = false;
                cc = Some(Cps::Range);
                continue;
            }
            self.unfetch();
            self.scan_base(1, base)?;
            n += 1;
            end_digit = true;
            if let Some(s) = cc {
                cc = Some(if s == Cps::Range {
                    Cps::Empty
                } else {
                    Cps::Start
                });
            }
        }
        Err(Error::new(E::InvalidCodePointValue))
    }

    /// `get_next_code_point`: the next code point of a `\x{a b}`
    /// sequence. `Ok(None)` at its `}`, `Err(Ok(()))`-like range markers
    /// are reported as [`NextCode::Range`].
    fn next_code_point(&mut self, in_cc: bool) -> Result<NextCode, Error> {
        let divide = |c: u32| c == ch(b' ') || c == ch(b'\n');
        while !self.end() {
            let c = self.fetch();
            if divide(c) {
                if self.end() {
                    return Err(Error::new(E::InvalidCodePointValue));
                }
            } else {
                if c == ch(b'}') {
                    return Ok(NextCode::End);
                } else if c == ch(b'-') && in_cc {
                    return Ok(NextCode::Range);
                }
                self.unfetch();
                break;
            }
        }
        let code = self.scan_base(1, self.base)?;
        Ok(NextCode::Code(code))
    }

    /// `fetch_name`, after the opening `<`, `'` or `(`. Returns the name's
    /// end and its reading as a number; `is_ref` allows numbers.
    fn fetch_name(&mut self, start: u32, is_ref: bool) -> Result<(usize, RefNum, i64), Error> {
        let src = self.p;
        let end_code = name_end_code(start);
        let mut name_end = self.s.len();
        let mut pnum_head = src;
        let mut num_type = RefNum::No;
        let mut sign: i64 = 1;
        let mut digits = 0;
        let mut r: Option<E> = None;
        if self.end() {
            return Err(Error::new(E::EmptyGroupName));
        }
        let mut c = self.fetch();
        if c == end_code {
            return Err(Error::new(E::EmptyGroupName));
        }
        if is_digit(c) {
            if is_ref {
                num_type = RefNum::Abs;
            } else {
                r = Some(E::InvalidGroupName);
            }
            digits += 1;
        } else if c == ch(b'-') || c == ch(b'+') {
            if is_ref {
                num_type = RefNum::Rel;
                sign = if c == ch(b'-') { -1 } else { 1 };
                pnum_head = self.p;
            } else {
                r = Some(E::InvalidGroupName);
            }
        } else if !unicode::is_word(c) {
            r = Some(E::InvalidCharInGroupName);
        }
        if r.is_none() {
            while !self.end() {
                name_end = self.p;
                c = self.fetch();
                if c == end_code || c == ch(b')') {
                    if num_type != RefNum::No && digits == 0 {
                        r = Some(E::InvalidGroupName);
                    }
                    break;
                }
                if num_type == RefNum::No {
                    if !unicode::is_word(c) {
                        r = Some(E::InvalidCharInGroupName);
                    }
                } else if is_digit(c) {
                    digits += 1;
                } else {
                    r = Some(if unicode::is_word(c) {
                        E::InvalidGroupName
                    } else {
                        E::InvalidCharInGroupName
                    });
                    num_type = RefNum::No;
                }
            }
            if c != end_code {
                return Err(self.named(E::InvalidGroupName, src, name_end));
            }
            // Oniguruma returns success here whatever the loop found: a
            // name is checked at its first character only.
            let _ = r;
            let mut num = 0;
            if num_type != RefNum::No {
                let Some(n) = self.number_in(pnum_head, name_end) else {
                    return Err(Error::new(E::TooBigNumber));
                };
                if n == 0 && num_type == RefNum::Rel {
                    return Err(self.named(E::InvalidGroupName, src, name_end));
                }
                num = i64::from(n) * sign;
            }
            return Ok((name_end, num_type, num));
        }
        while !self.end() {
            name_end = self.p;
            c = self.fetch();
            if c == end_code || c == ch(b')') {
                break;
            }
        }
        if self.end() {
            name_end = self.s.len();
        }
        Err(self.named(r.unwrap_or(E::InvalidGroupName), src, name_end))
    }

    /// `fetch_name_with_level`: a backreference's name or number, with an
    /// optional `+n`/`-n` level. Returns the name's end, its reading as a
    /// number, and the level.
    #[expect(
        clippy::many_single_char_names,
        reason = "Oniguruma's names (`c`, `p`, `r`), kept to stay comparable"
    )]
    fn fetch_name_with_level(
        &mut self,
        start: u32,
    ) -> Result<(usize, RefNum, i64, Option<i32>), Error> {
        let src = self.p;
        let end_code = name_end_code(start);
        let mut name_end = self.s.len();
        let mut pnum_head = src;
        let mut num_type = RefNum::No;
        let mut sign: i64 = 1;
        let mut digits = 0;
        let mut level = None;
        let mut r: Option<E> = None;
        let mut c: u32;
        if self.end() {
            return Err(Error::new(E::EmptyGroupName));
        }
        c = self.fetch();
        if c == end_code {
            return Err(Error::new(E::EmptyGroupName));
        }
        if is_digit(c) {
            num_type = RefNum::Abs;
            digits += 1;
        } else if c == ch(b'-') || c == ch(b'+') {
            num_type = RefNum::Rel;
            sign = if c == ch(b'-') { -1 } else { 1 };
            pnum_head = self.p;
        } else if !unicode::is_word(c) {
            r = Some(E::InvalidCharInGroupName);
        }
        while !self.end() {
            name_end = self.p;
            c = self.fetch();
            if c == end_code || c == ch(b')') || c == ch(b'+') || c == ch(b'-') {
                if num_type != RefNum::No && digits == 0 {
                    r = Some(E::InvalidGroupName);
                }
                break;
            }
            if num_type == RefNum::No {
                if !unicode::is_word(c) {
                    r = Some(E::InvalidCharInGroupName);
                }
            } else if is_digit(c) {
                digits += 1;
            } else {
                r = Some(E::InvalidGroupName);
                num_type = RefNum::No;
            }
        }
        if r.is_none() && c != end_code {
            let mut bad = true;
            if c == ch(b'+') || c == ch(b'-') {
                let flag: i32 = if c == ch(b'-') { -1 } else { 1 };
                if self.end() {
                    return Err(self.named(E::InvalidCharInGroupName, src, name_end));
                }
                let d = self.fetch();
                if is_digit(d) {
                    self.unfetch();
                    let Some(l) = self.scan_number() else {
                        return Err(Error::new(E::TooBigNumber));
                    };
                    level = Some(l.cast_signed() * flag);
                    if !self.end() {
                        let e = self.fetch();
                        if e == end_code {
                            bad = false;
                        }
                    }
                }
            }
            if bad {
                return Err(self.named(E::InvalidGroupName, src, self.s.len()));
            }
        }
        if let Some(kind) = r {
            return Err(self.named(kind, src, name_end));
        }
        let mut num = 0;
        if num_type != RefNum::No {
            let Some(n) = self.number_in(pnum_head, name_end) else {
                return Err(Error::new(E::TooBigNumber));
            };
            if n == 0 && num_type == RefNum::Rel {
                return Err(self.named(E::InvalidGroupName, src, self.s.len()));
            }
            num = i64::from(n) * sign;
        }
        Ok((name_end, num_type, num, level))
    }

    /// `backref_rel_to_abs`.
    fn rel_to_abs(&self, rel: i64) -> i64 {
        let n = i64::try_from(self.groups).unwrap_or(i64::MAX);
        if rel > 0 { n + rel } else { n + 1 + rel }
    }

    /// The groups a name refers to (`name_to_group_numbers`).
    fn name_groups(&self, name: &[u8]) -> Option<Vec<usize>> {
        self.names
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, g)| g.clone())
    }

    // --- tokens outside a class -------------------------------------

    /// `fetch_token`.
    #[expect(
        clippy::too_many_lines,
        reason = "Oniguruma's one switch over the characters; kept whole to stay comparable"
    )]
    fn fetch_token(&mut self) -> Result<Tok, Error> {
        if self.cp_continue {
            match self.next_code_point(false)? {
                NextCode::End => self.cp_continue = false,
                NextCode::Code(c) => return Ok(self.set(Tok::CodePoint(c))),
                NextCode::Range => unreachable!("not in a class"),
            }
        }
        loop {
            if self.end() {
                return Ok(self.set(Tok::Eot));
            }
            self.base = 0;
            self.backp = self.p;
            let c = self.fetch();
            if c == ch(b'\\') {
                if self.end() {
                    return Err(Error::new(E::EndPatternAtEscape));
                }
                self.backp = self.p;
                let c = self.fetch();
                self.escaped = true;
                let t = match u8::try_from(c).unwrap_or(0) {
                    b'w' => Tok::CharType {
                        ctype: ctype::WORD,
                        not: false,
                    },
                    b'W' => Tok::CharType {
                        ctype: ctype::WORD,
                        not: true,
                    },
                    b'b' => Tok::Anchor(Anchor::WordBoundary),
                    b'B' => Tok::Anchor(Anchor::NoWordBoundary),
                    b'y' => Tok::Anchor(Anchor::TextSegmentBoundary),
                    b'Y' => Tok::Anchor(Anchor::NoTextSegmentBoundary),
                    b's' => Tok::CharType {
                        ctype: ctype::SPACE,
                        not: false,
                    },
                    b'S' => Tok::CharType {
                        ctype: ctype::SPACE,
                        not: true,
                    },
                    b'd' => Tok::CharType {
                        ctype: ctype::DIGIT,
                        not: false,
                    },
                    b'D' => Tok::CharType {
                        ctype: ctype::DIGIT,
                        not: true,
                    },
                    b'K' => Tok::Keep,
                    b'R' => Tok::GeneralNewline,
                    b'N' => Tok::NoNewline,
                    b'O' => Tok::TrueAnychar,
                    b'X' => Tok::TextSegment,
                    b'A' => Tok::Anchor(Anchor::BeginBuf),
                    b'Z' => Tok::Anchor(Anchor::SemiEndBuf),
                    b'z' => Tok::Anchor(Anchor::EndBuf),
                    b'G' => Tok::Anchor(Anchor::BeginPosition),
                    b'o' => {
                        if !self.end() && self.peek_is(b'{') {
                            let prev = self.p;
                            if let Some(code) = self.brace_code_point(prev, 8, None)? {
                                return Ok(self.set(Tok::CodePoint(code)));
                            }
                        }
                        Tok::Str
                    }
                    b'x' => {
                        if self.end() {
                            Tok::Str
                        } else if self.peek_is(b'{') {
                            let prev = self.p;
                            match self.brace_code_point(prev, 16, None)? {
                                Some(code) => Tok::CodePoint(code),
                                None => Tok::Str,
                            }
                        } else {
                            let prev = self.p;
                            let code = self.scan_hex(0, 2)?;
                            let code = if self.p == prev { 0 } else { code };
                            self.base = 16;
                            Tok::CrudeByte(u8::try_from(code).unwrap_or(u8::MAX))
                        }
                    }
                    b'1'..=b'9' => {
                        self.unfetch();
                        let prev = self.p;
                        match self.scan_number() {
                            Some(r)
                                if r <= MAX_BACKREF_NUM
                                    && (r as usize <= self.groups || r <= 9) =>
                            {
                                Tok::Backref {
                                    groups: vec![r as usize],
                                    level: None,
                                }
                            }
                            _ => {
                                if c == ch(b'8') || c == ch(b'9') {
                                    self.p = prev;
                                    self.inc();
                                    Tok::Str
                                } else {
                                    self.p = prev;
                                    self.octal_escape(c)?
                                }
                            }
                        }
                    }
                    b'0' => self.octal_escape(c)?,
                    b'k' => {
                        if self.end() {
                            Tok::Str
                        } else {
                            let d = self.fetch();
                            if d == ch(b'<') || d == ch(b'\'') {
                                self.backref_by_name(d)?
                            } else {
                                self.unfetch();
                                Tok::Str
                            }
                        }
                    }
                    b'g' => {
                        if self.end() {
                            Tok::Str
                        } else {
                            let d = self.fetch();
                            if d == ch(b'<') || d == ch(b'\'') {
                                self.fetch_name(d, true)?;
                                Tok::Call
                            } else {
                                self.unfetch();
                                Tok::Str
                            }
                        }
                    }
                    b'Q' => Tok::QuoteOpen,
                    b'p' | b'P' => {
                        if self.peek_is(b'{') {
                            self.inc();
                            let mut not = c == ch(b'P');
                            if !self.end() {
                                let d = self.fetch();
                                if d == ch(b'^') {
                                    not = !not;
                                } else {
                                    self.unfetch();
                                }
                            }
                            Tok::Prop { not }
                        } else {
                            Tok::Str
                        }
                    }
                    _ => {
                        self.unfetch();
                        let c2 = self.fetch_escaped_value()?;
                        if c2 == c {
                            self.p = self.backp + enc::len(self.s, self.backp);
                            Tok::Str
                        } else {
                            Tok::CodePoint(c2)
                        }
                    }
                };
                return Ok(self.set(t));
            }
            self.escaped = false;
            let t = match u8::try_from(c).unwrap_or(0) {
                b'.' => Tok::AnyChar,
                b'*' => Tok::Repeat(self.greedy_check(0, None)),
                b'+' => Tok::Repeat(self.greedy_check(1, None)),
                b'?' => Tok::Repeat(self.greedy_check(0, Some(1))),
                b'{' => match self.fetch_interval()? {
                    Some((rep, _fixed)) => Tok::Repeat(self.greedy_check2(rep)),
                    None => Tok::Str,
                },
                b'|' => Tok::Alt,
                b'(' => {
                    if let Some(t) = self.lparen()? {
                        t
                    } else {
                        continue;
                    }
                }
                b')' => Tok::Close,
                b'^' => Tok::Anchor(if self.opts.singleline {
                    Anchor::BeginBuf
                } else {
                    Anchor::BeginLine
                }),
                b'$' => Tok::Anchor(if self.opts.singleline {
                    Anchor::SemiEndBuf
                } else {
                    Anchor::EndLine
                }),
                b'[' => Tok::OpenCc,
                b'#' if self.opts.extend => {
                    while !self.end() {
                        if self.fetch() == ch(b'\n') {
                            break;
                        }
                    }
                    continue;
                }
                b' ' | b'\t' | b'\n' | b'\r' | 0x0c if self.opts.extend => continue,
                _ => Tok::Str,
            };
            return Ok(self.set(t));
        }
    }

    fn set(&mut self, t: Tok) -> Tok {
        self.tok = t.clone();
        t
    }

    /// The octal escape of `\0…` and of `\1…` that is no backreference.
    fn octal_escape(&mut self, c: u32) -> Result<Tok, Error> {
        let prev = self.p;
        let code = self.scan_octal(0, if c == ch(b'0') { 2 } else { 3 })?;
        let code = if self.p == prev { 0 } else { code };
        self.base = 8;
        // `r >= 256` tests the scanner's status, not the code: 0777 is
        // the byte 0xff.
        Ok(Tok::CrudeByte((code & 0xff) as u8))
    }

    /// `\k<name>`, `\k<n>`, `\k<-n>`, with an optional level.
    fn backref_by_name(&mut self, d: u32) -> Result<Tok, Error> {
        let prev = self.p;
        let (name_end, num_type, num, level) = self.fetch_name_with_level(d)?;
        if num_type == RefNum::No {
            let Some(groups) = self.name_groups(&self.s[prev..name_end]) else {
                return Err(self.named(E::UndefinedNameReference, prev, name_end));
            };
            return Ok(Tok::Backref { groups, level });
        }
        let n = if num_type == RefNum::Rel {
            self.rel_to_abs(num)
        } else {
            num
        };
        if n <= 0 {
            return Err(Error::new(E::InvalidBackref));
        }
        Ok(Tok::Backref {
            groups: vec![usize::try_from(n).unwrap_or(usize::MAX)],
            level,
        })
    }

    /// `greedy_check`: a `?` makes the repeat lazy, a `+` possessive.
    fn greedy_check(&mut self, lower: u32, upper: Option<u32>) -> Rep {
        self.greedy_check2(Rep {
            lower,
            upper,
            greedy: true,
            possessive: false,
            interval: false,
        })
    }

    fn greedy_check2(&mut self, mut rep: Rep) -> Rep {
        if self.peek_is(b'?') {
            self.fetch();
            rep.greedy = false;
        } else {
            rep.greedy = true;
            if self.peek_is(b'+') {
                self.fetch();
                rep.possessive = true;
            }
        }
        rep
    }

    /// What follows `(`: a comment (skipped, `None`), a subexpression call,
    /// or the opening of a group.
    fn lparen(&mut self) -> Result<Option<Tok>, Error> {
        if !self.peek_is(b'?') {
            return Ok(Some(Tok::Open));
        }
        let prev = self.p;
        self.inc();
        if self.end() {
            self.p = prev;
            return Ok(Some(Tok::Open));
        }
        let c = self.peek();
        if c == ch(b'#') {
            self.fetch();
            loop {
                if self.end() {
                    return Err(Error::new(E::EndPatternInGroup));
                }
                let c = self.fetch();
                if c == ch(b'\\') {
                    if !self.end() {
                        self.fetch();
                    }
                } else if c == ch(b')') {
                    break;
                }
            }
            return Ok(None);
        }
        // `ONIG_SYN_OP2_QMARK_PERL_SUBEXP_CALL`.
        if c == ch(b'&') {
            self.inc();
            self.fetch_name(ch(b'('), false)?;
            return Ok(Some(Tok::Call));
        }
        if c == ch(b'R') {
            self.inc();
            if !self.peek_is(b')') {
                return Err(Error::new(E::UndefinedGroupOption));
            }
            return Ok(Some(Tok::Call));
        }
        if c == ch(b'-') || c == ch(b'+') {
            if !self.end() {
                self.inc();
                if !self.end() && unicode::is_ctype(self.peek(), ctype::DIGIT) {
                    self.unfetch();
                    return self.call_by_number().map(Some);
                }
            }
            self.p = prev;
            return Ok(Some(Tok::Open));
        }
        if unicode::is_ctype(c, ctype::DIGIT) {
            return self.call_by_number().map(Some);
        }
        self.p = prev;
        Ok(Some(Tok::Open))
    }

    /// `(?1)`, `(?+1)`, `(?-1)`.
    fn call_by_number(&mut self) -> Result<Tok, Error> {
        let name = self.p;
        let (name_end, num_type, num) = self.fetch_name(ch(b'('), true)?;
        if num_type == RefNum::No {
            return Err(self.named(E::InvalidGroupName, name, name_end));
        }
        if num_type == RefNum::Rel && self.rel_to_abs(num) < 0 {
            return Err(self.named(E::UndefinedGroupReference, name, name_end));
        }
        Ok(Tok::Call)
    }

    // --- classes -----------------------------------------------------

    /// `fetch_token_cc`.
    #[expect(
        clippy::too_many_lines,
        reason = "Oniguruma's one switch over the class characters; kept whole to stay comparable"
    )]
    fn fetch_token_cc(&mut self, state: CState) -> Result<CcTok, Error> {
        if self.cp_continue {
            match self.next_code_point(true)? {
                NextCode::End => self.cp_continue = false,
                NextCode::Range => return Ok(CcTok::Range),
                NextCode::Code(c) => return Ok(CcTok::CodePoint(c)),
            }
        }
        if self.end() {
            return Ok(CcTok::Eot);
        }
        let c = self.fetch();
        self.base = 0;
        if c == ch(b']') {
            return Ok(CcTok::Close);
        }
        if c == ch(b'-') {
            return Ok(CcTok::Range);
        }
        if c == ch(b'\\') {
            if self.end() {
                return Err(Error::new(E::EndPatternAtEscape));
            }
            let c = self.fetch();
            let t = match u8::try_from(c).unwrap_or(0) {
                b'w' => CcTok::CharType {
                    ctype: ctype::WORD,
                    not: false,
                },
                b'W' => CcTok::CharType {
                    ctype: ctype::WORD,
                    not: true,
                },
                b'd' => CcTok::CharType {
                    ctype: ctype::DIGIT,
                    not: false,
                },
                b'D' => CcTok::CharType {
                    ctype: ctype::DIGIT,
                    not: true,
                },
                b's' => CcTok::CharType {
                    ctype: ctype::SPACE,
                    not: false,
                },
                b'S' => CcTok::CharType {
                    ctype: ctype::SPACE,
                    not: true,
                },
                b'p' | b'P' => {
                    if !self.end() && self.peek_is(b'{') {
                        self.inc();
                        let mut not = c == ch(b'P');
                        if !self.end() {
                            let d = self.fetch();
                            if d == ch(b'^') {
                                not = !not;
                            } else {
                                self.unfetch();
                            }
                        }
                        CcTok::Prop { not }
                    } else {
                        CcTok::Char {
                            code: c,
                            escaped: true,
                        }
                    }
                }
                b'o' => {
                    if !self.end() && self.peek_is(b'{') {
                        let prev = self.p;
                        if let Some(code) = self.brace_code_point(prev, 8, Some(state))? {
                            return Ok(CcTok::CodePoint(code));
                        }
                    }
                    CcTok::Char {
                        code: c,
                        escaped: true,
                    }
                }
                b'x' => {
                    if self.end() {
                        CcTok::Char {
                            code: c,
                            escaped: true,
                        }
                    } else if self.peek_is(b'{') {
                        let prev = self.p;
                        match self.brace_code_point(prev, 16, Some(state))? {
                            Some(code) => CcTok::CodePoint(code),
                            None => CcTok::Char {
                                code: c,
                                escaped: true,
                            },
                        }
                    } else {
                        let prev = self.p;
                        let code = self.scan_hex(0, 2)?;
                        let code = if self.p == prev { 0 } else { code };
                        self.base = 16;
                        CcTok::CrudeByte(u8::try_from(code).unwrap_or(u8::MAX))
                    }
                }
                b'0'..=b'7' => {
                    self.unfetch();
                    let prev = self.p;
                    let code = self.scan_octal(0, 3)?;
                    if code >= 256 {
                        return Err(Error::new(E::TooBigNumber));
                    }
                    let code = if self.p == prev { 0 } else { code };
                    self.base = 8;
                    CcTok::CrudeByte(u8::try_from(code).unwrap_or(u8::MAX))
                }
                _ => {
                    self.unfetch();
                    let c2 = self.fetch_escaped_value()?;
                    if c2 == c {
                        CcTok::Char {
                            code: c,
                            escaped: true,
                        }
                    } else {
                        CcTok::CodePoint(c2)
                    }
                }
            };
            return Ok(t);
        }
        if c == ch(b'[') && self.peek_is(b':') {
            self.backp = self.p;
            self.inc();
            if self.is_posix_bracket_start() {
                return Ok(CcTok::PosixOpen);
            }
            self.unfetch();
            // No `ONIG_SYN_OP2_CCLASS_SET_OP`: a `[` is a character.
        }
        Ok(CcTok::Char {
            code: c,
            escaped: false,
        })
    }

    /// `is_posix_bracket_start`, after `[:`.
    fn is_posix_bracket_start(&self) -> bool {
        let mut n = 0;
        let mut p = self.p;
        while p < self.s.len() {
            let x = enc::code(self.s, p);
            p += enc::len(self.s, p);
            if x == ch(b':') {
                if p < self.s.len() && enc::code(self.s, p) == ch(b']') {
                    return n != 0;
                }
                return false;
            } else if x == ch(b'^') && n == 0 {
            } else if !unicode::is_ctype(x, ctype::ALPHA) {
                break;
            }
            n += 1;
        }
        false
    }

    /// `prs_posix_bracket`, after `[:`.
    fn prs_posix_bracket(&mut self, cc: &mut Class) -> Result<(), Error> {
        const PBS: [(&[u8], usize); 14] = [
            (b"alnum", ctype::ALNUM),
            (b"alpha", ctype::ALPHA),
            (b"blank", ctype::BLANK),
            (b"cntrl", ctype::CNTRL),
            (b"digit", ctype::DIGIT),
            (b"graph", ctype::GRAPH),
            (b"lower", ctype::LOWER),
            (b"print", ctype::PRINT),
            (b"punct", ctype::PUNCT),
            (b"space", ctype::SPACE),
            (b"upper", ctype::UPPER),
            (b"xdigit", ctype::XDIGIT),
            (b"ascii", ctype::ASCII),
            (b"word", ctype::WORD),
        ];
        let not = if self.peek_is(b'^') {
            self.inc();
            true
        } else {
            false
        };
        for (name, ct) in PBS {
            if self.s[self.p..].starts_with(name) {
                let at = self.p + name.len();
                if !self.s[at..].starts_with(b":]") {
                    break;
                }
                add_ctype_to_cc(cc, ct, not);
                self.p = at + 2;
                return Ok(());
            }
        }
        Err(Error::new(E::InvalidPosixBracketType))
    }

    /// `fetch_char_property_to_ctype`, after `\p{`.
    fn fetch_char_property(&mut self) -> Result<usize, Error> {
        let start = self.p;
        while !self.end() {
            let prev = self.p;
            let c = self.fetch();
            if c == ch(b'}') {
                return unicode::property(&self.s[start..prev])
                    .ok_or_else(|| self.named(E::InvalidCharPropertyName, start, prev));
            } else if c == ch(b'(') || c == ch(b')') || c == ch(b'{') || c == ch(b'|') {
                break;
            }
        }
        Err(Error::new(E::EndPatternWithUnmatchedParenthesis))
    }

    /// `prs_cc`, after `[`.
    #[expect(
        clippy::too_many_lines,
        clippy::many_single_char_names,
        reason = "Oniguruma's class state machine and names; kept whole to stay comparable"
    )]
    fn prs_cc(&mut self) -> Result<Class, Error> {
        self.deeper()?;
        let mut state = CState::Start;
        let mut t = self.fetch_token_cc(state)?;
        let neg = if matches!(
            t,
            CcTok::Char {
                code: 0x5e,
                escaped: false
            }
        ) {
            t = self.fetch_token_cc(state)?;
            true
        } else {
            false
        };
        if t == CcTok::Close {
            if !self.code_exist(ch(b']')) {
                return Err(Error::new(E::EmptyCharClass));
            }
            t = CcTok::Char {
                code: ch(b']'),
                escaped: false,
            };
        }
        let mut cc = Class::default();
        let mut curr_code: u32 = 0;
        let mut curr_type = CVal::Undef;
        while t != CcTok::Close {
            let mut fetched = false;
            // What a value token contributes: (code, type).
            let mut value: Option<(u32, CVal)> = None;
            match t {
                CcTok::Char { code, .. } => {
                    value = Some((code, char_type(code)?));
                }
                CcTok::CrudeByte(b) => {
                    // Crude bytes join into one UTF-8 character.
                    let base = self.base;
                    let save = self.p;
                    let mut buf = vec![b];
                    let need = enc::len(&[b, 0, 0, 0], 0).max(1);
                    let mut i = 1;
                    let mut next = None;
                    while i < 4 {
                        let save_i = self.p;
                        let n = self.fetch_token_cc(CState::Complete)?;
                        match n {
                            CcTok::CrudeByte(x) if self.base == base => buf.push(x),
                            other => {
                                next = Some((other, save_i));
                                break;
                            }
                        }
                        i += 1;
                    }
                    let len = need;
                    if i < len {
                        return Err(Error::new(E::TooShortMultiByteString));
                    } else if i > len {
                        // Read too many: go back and read just enough.
                        self.p = save;
                        for _ in 1..len {
                            self.fetch_token_cc(CState::Complete)?;
                        }
                        next = None;
                    }
                    if let Some((n, _)) = next {
                        t = n;
                        fetched = true;
                    }
                    let _ = base;
                    if len == 1 {
                        value = Some((u32::from(buf[0]), CVal::Sb));
                    } else {
                        let bytes = &buf[..len];
                        if !enc::is_valid(bytes) {
                            return Err(Error::new(E::InvalidCodePointValue));
                        }
                        value = Some((enc::code(bytes, 0), CVal::Mb));
                    }
                }
                CcTok::CodePoint(code) => {
                    let Some(len) = enc::code_len(code) else {
                        return Err(Error::new(E::InvalidCodePointValue));
                    };
                    value = Some((code, if len == 1 { CVal::Sb } else { CVal::Mb }));
                }
                CcTok::PosixOpen => {
                    self.prs_posix_bracket(&mut cc)?;
                    cprop_next(&mut cc, &mut curr_code, &mut curr_type, &mut state)?;
                }
                CcTok::CharType { ctype, not } => {
                    add_ctype_to_cc(&mut cc, ctype, not);
                    cprop_next(&mut cc, &mut curr_code, &mut curr_type, &mut state)?;
                }
                CcTok::Prop { not } => {
                    let ctype = self.fetch_char_property()?;
                    add_ctype_to_cc(&mut cc, ctype, not);
                    cprop_next(&mut cc, &mut curr_code, &mut curr_type, &mut state)?;
                }
                CcTok::Range => match state {
                    CState::Value => {
                        let n = self.fetch_token_cc(CState::Range)?;
                        fetched = true;
                        t = n;
                        if n == CcTok::Close || curr_type == CVal::CProp {
                            // `[x-]`, and `[\w-]`
                            // (`ONIG_SYN_ALLOW_CHAR_TYPE_FOLLOWED_BY_MINUS_IN_CC`).
                            char_next(
                                &mut cc,
                                &mut curr_code,
                                ch(b'-'),
                                CVal::Sb,
                                &mut curr_type,
                                &mut state,
                            )?;
                        } else {
                            state = CState::Range;
                        }
                    }
                    CState::Start => {
                        // `[-xa]`
                        let n = self.fetch_token_cc(CState::Value)?;
                        fetched = true;
                        t = n;
                        char_next(
                            &mut cc,
                            &mut curr_code,
                            ch(b'-'),
                            CVal::Sb,
                            &mut curr_type,
                            &mut state,
                        )?;
                    }
                    CState::Range => {
                        // `[!--]`
                        value = Some((ch(b'-'), CVal::Sb));
                    }
                    CState::Complete => {
                        // `[a-b-]`, and `[0-9-a]` as `[0-9\-a]`
                        // (`ONIG_SYN_ALLOW_DOUBLE_RANGE_OP_IN_CC`).
                        let n = self.fetch_token_cc(CState::Value)?;
                        fetched = true;
                        t = n;
                        char_next(
                            &mut cc,
                            &mut curr_code,
                            ch(b'-'),
                            CVal::Sb,
                            &mut curr_type,
                            &mut state,
                        )?;
                    }
                },
                CcTok::Eot => return Err(Error::new(E::PrematureEndOfCharClass)),
                CcTok::Close => unreachable!("loop condition"),
            }
            if let Some((code, ty)) = value {
                char_next(
                    &mut cc,
                    &mut curr_code,
                    code,
                    ty,
                    &mut curr_type,
                    &mut state,
                )?;
            }
            if !fetched {
                t = self.fetch_token_cc(state)?;
            }
        }
        if state == CState::Value {
            flush_value(&mut cc, curr_code, curr_type)?;
        }
        cc.not = neg;
        self.depth -= 1;
        Ok(cc)
    }

    /// `code_exist_check`: whether `c` occurs unescaped further on.
    fn code_exist(&self, c: u32) -> bool {
        let mut p = self.p;
        let mut in_esc = false;
        while p < self.s.len() {
            if in_esc {
                in_esc = false;
                p += enc::len(self.s, p);
                continue;
            }
            let x = enc::code(self.s, p);
            p += enc::len(self.s, p);
            if x == c {
                return true;
            }
            if x == ch(b'\\') {
                in_esc = true;
            }
        }
        false
    }

    // --- groups and expressions -------------------------------------

    /// `prs_bag`, after `(`. Returns the node and what it is: a plain
    /// group (`1`), an isolated option (`2`) or anything else (`0`).
    #[expect(
        clippy::too_many_lines,
        reason = "Oniguruma's one switch over the group kinds; kept whole to stay comparable"
    )]
    fn prs_bag(&mut self) -> Result<(Node, u8, Options), Error> {
        if self.end() {
            return Err(Error::new(E::EndPatternWithUnmatchedParenthesis));
        }
        let mut option = self.opts;
        if self.peek_is(b'?') {
            self.inc();
            if self.end() {
                return Err(Error::new(E::EndPatternInGroup));
            }
            let mut c = self.fetch();
            match u8::try_from(c).unwrap_or(0) {
                b':' => {
                    let mut t = self.fetch_token()?;
                    let (n, _) = self.prs_alts(&mut t, Term::Close, false)?;
                    return Ok((n, 1, option));
                }
                b'=' | b'!' => {
                    let body = self.group_body()?;
                    return Ok((
                        Node::Look {
                            behind: false,
                            not: c == ch(b'!'),
                            body: Box::new(body),
                        },
                        0,
                        option,
                    ));
                }
                b'>' => {
                    let body = self.group_body()?;
                    return Ok((Node::Atomic(Box::new(body)), 0, option));
                }
                b'\'' => return self.named_group(c).map(|n| (n, 0, option)),
                b'<' => {
                    if self.end() {
                        return Err(Error::new(E::EndPatternWithUnmatchedParenthesis));
                    }
                    c = self.fetch();
                    if c == ch(b'=') || c == ch(b'!') {
                        let body = self.group_body()?;
                        return Ok((
                            Node::Look {
                                behind: true,
                                not: c == ch(b'!'),
                                body: Box::new(body),
                            },
                            0,
                            option,
                        ));
                    }
                    self.unfetch();
                    return self.named_group(ch(b'<')).map(|n| (n, 0, option));
                }
                b'~' | b'{' => return Err(Error::new(E::Unsupported)),
                b'(' => return self.if_else().map(|n| (n, 0, option)),
                b'P' | b'W' | b'D' | b'S' | b'y' => {
                    return Err(Error::new(E::UndefinedGroupOption));
                }
                b'a' | b'-' | b'i' | b'm' | b's' | b'x' => {
                    let mut neg = false;
                    loop {
                        match u8::try_from(c).unwrap_or(0) {
                            b':' | b')' => {}
                            b'-' => neg = true,
                            b'x' => option.extend = !neg,
                            b'i' => option.ignorecase = !neg,
                            // `ONIG_SYN_OP2_OPTION_PERL`: `s` is
                            // Oniguruma's multiline, `m` negates
                            // singleline.
                            b's' => option.multiline = !neg,
                            b'm' => option.singleline = neg,
                            _ => return Err(Error::new(E::UndefinedGroupOption)),
                        }
                        if c == ch(b')') {
                            return Ok((Node::Empty, 2, option));
                        } else if c == ch(b':') {
                            let prev = self.opts;
                            self.opts = option;
                            let mut t = self.fetch_token()?;
                            let r = self.prs_alts(&mut t, Term::Close, false);
                            self.opts = prev;
                            let (n, _) = r?;
                            return Ok((Node::Group(Box::new(n)), 0, option));
                        }
                        if self.end() {
                            return Err(Error::new(E::EndPatternInGroup));
                        }
                        c = self.fetch();
                    }
                }
                _ => return Err(Error::new(E::UndefinedGroupOption)),
            }
        }
        if self.peek_is(b'*') {
            return Err(Error::new(E::Unsupported));
        }
        self.groups += 1;
        let num = self.groups;
        self.group_nodes.push(false);
        let body = self.group_body()?;
        self.group_nodes[num - 1] = true;
        Ok((
            Node::Capture {
                num,
                body: Box::new(body),
            },
            0,
            option,
        ))
    }

    /// A group's alternatives up to its `)`.
    fn group_body(&mut self) -> Result<Node, Error> {
        let mut t = self.fetch_token()?;
        Ok(self.prs_alts(&mut t, Term::Close, false)?.0)
    }

    /// `(?<name>…)` and `(?'name'…)`, after the opening delimiter.
    fn named_group(&mut self, c: u32) -> Result<Node, Error> {
        let name = self.p;
        let (name_end, _, _) = self.fetch_name(c, false)?;
        self.groups += 1;
        let num = self.groups;
        self.group_nodes.push(false);
        let key = self.s[name..name_end].to_vec();
        // `ONIG_SYN_ALLOW_MULTIPLEX_DEFINITION_NAME`.
        match self.names.iter_mut().find(|(n, _)| *n == key) {
            Some((_, g)) => g.push(num),
            None => self.names.push((key, vec![num])),
        }
        let body = self.group_body()?;
        self.group_nodes[num - 1] = true;
        Ok(Node::Capture {
            num,
            body: Box::new(body),
        })
    }

    /// `(?(cond)then|else)`, after `(?(`.
    fn if_else(&mut self) -> Result<Node, Error> {
        if self.end() {
            return Err(Error::new(E::EndPatternInGroup));
        }
        let c = self.fetch();
        if self.end() {
            return Err(Error::new(E::EndPatternInGroup));
        }
        let mut checker = false;
        let cond;
        let enclosed = c == ch(b'<') || c == ch(b'\'');
        if is_digit(c) || c == ch(b'-') || c == ch(b'+') || enclosed {
            if !enclosed {
                self.unfetch();
            }
            let prev = self.p;
            let start = if enclosed { c } else { ch(b'(') };
            match self.fetch_name_with_level(start) {
                Err(e) => {
                    if enclosed {
                        return Err(e);
                    }
                    self.p = prev;
                    cond = self.any_condition()?;
                }
                Ok((name_end, num_type, num, _level)) => {
                    checker = true;
                    let groups = if num_type == RefNum::No {
                        let Some(g) = self.name_groups(&self.s[prev..name_end]) else {
                            return Err(self.named(E::UndefinedNameReference, prev, name_end));
                        };
                        g
                    } else {
                        let n = if num_type == RefNum::Rel {
                            self.rel_to_abs(num)
                        } else {
                            num
                        };
                        if n <= 0 {
                            return Err(Error::new(E::InvalidBackref));
                        }
                        vec![usize::try_from(n).unwrap_or(usize::MAX)]
                    };
                    cond = Node::BackrefCheck { groups };
                    if enclosed {
                        if self.end() {
                            return Err(Error::new(E::EndPatternInGroup));
                        }
                        if self.fetch() != ch(b')') {
                            return Err(Error::new(E::EndPatternInGroup));
                        }
                    }
                }
            }
        } else if c == ch(b'?') || c == ch(b'*') {
            return Err(Error::new(E::Unsupported));
        } else {
            self.unfetch();
            cond = self.any_condition()?;
        }
        if self.end() {
            return Err(Error::new(E::EndPatternInGroup));
        }
        if self.peek_is(b')') {
            if !checker {
                return Err(Error::new(E::InvalidIfElseSyntax));
            }
            self.fetch();
            return Ok(cond);
        }
        let then_empty = if self.peek_is(b'|') {
            self.fetch();
            true
        } else {
            false
        };
        let mut t = self.fetch_token()?;
        let (target, _) = self.prs_alts(&mut t, Term::Close, true)?;
        let (then, els) = if then_empty {
            (None, Some(target))
        } else if let Node::Alt(mut alts) = target {
            let first = alts.remove(0);
            let rest = if alts.len() == 1 {
                alts.remove(0)
            } else {
                Node::Alt(alts)
            };
            (Some(first), Some(rest))
        } else {
            (Some(target), None)
        };
        Ok(Node::IfElse {
            cond: Box::new(cond),
            then: then.map(Box::new),
            els: els.map(Box::new),
        })
    }

    /// A condition that is a pattern of its own, up to its `)`.
    fn any_condition(&mut self) -> Result<Node, Error> {
        let mut t = self.fetch_token()?;
        Ok(self.prs_alts(&mut t, Term::Close, false)?.0)
    }

    /// `prs_exp`: one item and its quantifiers. Returns the node and the
    /// token after it.
    #[expect(
        clippy::too_many_lines,
        reason = "Oniguruma's one switch over the token kinds; kept whole to stay comparable"
    )]
    fn prs_exp(&mut self, t: &mut Tok, term: Term, group_head: bool) -> Result<Node, Error> {
        loop {
            let mut group = 0u8;
            if is_term(t, term) || matches!(t, Tok::Alt | Tok::Eot) {
                return Ok(Node::Empty);
            }
            let mut node = match t.clone() {
                Tok::Open => {
                    let (n, kind, option) = self.prs_bag()?;
                    if kind == 1 {
                        // `(?:…)` is its body itself; only at the head of
                        // an if-else branch is it wrapped, as a list
                        // (`node_new_group`), which no quantifier rejects.
                        if group_head {
                            group = 2;
                            Node::Group(Box::new(n))
                        } else {
                            group = 1;
                            n
                        }
                    } else if kind == 2 {
                        // `ONIG_SYN_ISOLATED_OPTION_CONTINUE_BRANCH`.
                        self.opts = option;
                        *t = self.fetch_token()?;
                        continue;
                    } else {
                        n
                    }
                }
                Tok::Close => return Err(Error::new(E::UnmatchedCloseParenthesis)),
                Tok::Str => {
                    let mut bytes = self.s[self.backp..self.p].to_vec();
                    let icase = self.opts.ignorecase;
                    loop {
                        *t = self.fetch_token()?;
                        if *t != Tok::Str {
                            break;
                        }
                        bytes.extend_from_slice(&self.s[self.backp..self.p]);
                    }
                    let node = Node::Str { bytes, icase };
                    return self.quantifiers(node, t, group, term);
                }
                Tok::CrudeByte(b) => {
                    let mut bytes = vec![b];
                    let icase = self.opts.ignorecase;
                    loop {
                        if bytes.len() == enc::len(&bytes, 0).max(1)
                            && bytes.len() == lead_len_of(bytes[0])
                        {
                            *t = self.fetch_token()?;
                            break;
                        }
                        let n = self.fetch_token()?;
                        let Tok::CrudeByte(x) = n else {
                            return Err(Error::new(E::TooShortMultiByteString));
                        };
                        bytes.push(x);
                    }
                    if !enc::is_valid(&bytes) {
                        return Err(Error::new(E::InvalidCodePointValue));
                    }
                    let node = Node::Str { bytes, icase };
                    return self.quantifiers(node, t, group, term);
                }
                Tok::CodePoint(code) => {
                    let mut bytes = Vec::new();
                    if enc::code_len(code).is_none() {
                        return Err(Error::new(E::InvalidCodePointValue));
                    }
                    enc::push_code(code, &mut bytes);
                    Node::Str {
                        bytes,
                        icase: self.opts.ignorecase,
                    }
                }
                Tok::QuoteOpen => {
                    let start = self.p;
                    let (qend, next) = match find_quote_end(self.s, start) {
                        Some((a, b)) => (a, b),
                        None => (self.s.len(), self.s.len()),
                    };
                    self.p = next;
                    Node::Str {
                        bytes: self.s[start..qend].to_vec(),
                        icase: self.opts.ignorecase,
                    }
                }
                Tok::CharType { ctype, not } => {
                    if ctype == ctype::WORD {
                        Node::Word { not }
                    } else {
                        let mut cc = Class::default();
                        add_ctype_to_cc(&mut cc, ctype, false);
                        cc.not = not;
                        Node::Class(Box::new(cc))
                    }
                }
                Tok::Prop { not } => {
                    let ctype = self.fetch_char_property()?;
                    if ctype == ctype::WORD {
                        Node::Word { not }
                    } else {
                        let mut cc = Class::default();
                        add_ctype_to_cc(&mut cc, ctype, false);
                        cc.not = not;
                        Node::Class(Box::new(cc))
                    }
                }
                Tok::OpenCc => {
                    let cc = self.prs_cc()?;
                    if self.opts.ignorecase {
                        apply_case_fold(cc)
                    } else {
                        Node::Class(Box::new(cc))
                    }
                }
                Tok::AnyChar => Node::Any {
                    multiline: self.opts.multiline,
                },
                Tok::Backref { groups, level } => {
                    // `node_new_backref`: a group whose `)` has not been
                    // read yet makes the reference a recursion.
                    let recursive = groups
                        .iter()
                        .any(|&g| g <= self.groups && !self.group_nodes[g - 1]);
                    Node::Backref {
                        groups,
                        icase: self.opts.ignorecase,
                        level,
                        recursive,
                    }
                }
                Tok::Call => return Err(Error::new(E::Unsupported)),
                Tok::Anchor(a) => Node::Anchor(a),
                Tok::Repeat(_) => {
                    return Err(Error::new(E::TargetOfRepeatOperatorNotSpecified));
                }
                Tok::Keep => Node::Keep,
                Tok::GeneralNewline => general_newline(),
                Tok::NoNewline => Node::Any { multiline: false },
                Tok::TrueAnychar => Node::Any { multiline: true },
                Tok::TextSegment => text_segment(),
                Tok::Alt | Tok::Eot => unreachable!("handled above"),
            };
            *t = self.fetch_token()?;
            node = self.quantifiers(node, t, group, term)?;
            return Ok(node);
        }
    }

    /// The `repeat:` loop of `prs_exp`: quantifiers after a node.
    fn quantifiers(
        &mut self,
        mut node: Node,
        t: &mut Tok,
        mut group: u8,
        _term: Term,
    ) -> Result<Node, Error> {
        // A string's quantifier takes its last character only.
        let mut head: Option<Node> = None;
        while let Tok::Repeat(rep) = t.clone() {
            if is_invalid_quantifier_target(&node) {
                return Err(Error::new(E::TargetOfRepeatOperatorInvalid));
            }
            self.deeper()?;
            self.depth -= 1;
            let target = if group == 2 {
                match node {
                    Node::Group(n) => *n,
                    n => n,
                }
            } else {
                node
            };
            let (q, split) = assign_quantifier_body(rep, target, group)?;
            if let Some(h) = split {
                head = Some(match head.take() {
                    Some(prev) => concat(prev, h),
                    None => h,
                });
            }
            node = q;
            group = 0;
            *t = self.fetch_token()?;
        }
        Ok(match head {
            Some(h) => concat(h, node),
            None => node,
        })
    }

    /// `prs_branch`.
    fn prs_branch(&mut self, t: &mut Tok, term: Term, group_head: bool) -> Result<Node, Error> {
        self.deeper()?;
        let first = self.prs_exp(t, term, group_head)?;
        let node = if is_term(t, term) || matches!(t, Tok::Eot | Tok::Alt) {
            first
        } else {
            let mut items = vec![first];
            while !(is_term(t, term) || matches!(t, Tok::Eot | Tok::Alt)) {
                let n = self.prs_exp(t, term, false)?;
                match n {
                    Node::List(xs) => items.extend(xs),
                    n => items.push(n),
                }
            }
            Node::List(items)
        };
        self.depth -= 1;
        Ok(node)
    }

    /// `prs_alts`. Returns the alternatives and whether the run ended at
    /// `term`.
    fn prs_alts(
        &mut self,
        t: &mut Tok,
        term: Term,
        group_head: bool,
    ) -> Result<(Node, bool), Error> {
        self.deeper()?;
        let save = self.opts;
        let first = self.prs_branch(t, term, group_head)?;
        let node = if is_term(t, term) {
            first
        } else if *t == Tok::Alt {
            let mut alts = vec![first];
            while *t == Tok::Alt {
                *t = self.fetch_token()?;
                alts.push(self.prs_branch(t, term, false)?);
            }
            if !is_term(t, term) {
                return Err(Self::unmatched(term));
            }
            Node::Alt(alts)
        } else {
            return Err(Self::unmatched(term));
        };
        self.opts = save;
        self.depth -= 1;
        Ok((node, true))
    }

    fn unmatched(term: Term) -> Error {
        match term {
            Term::Close => Error::new(E::EndPatternWithUnmatchedParenthesis),
            Term::Eot => Error::new(E::UnmatchedCloseParenthesis),
        }
    }
}

/// The states of a `\x{…}` sequence's check (`CPS_*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cps {
    Empty,
    Start,
    Range,
}

/// What `get_next_code_point` found.
enum NextCode {
    End,
    Range,
    Code(u32),
}

fn is_term(t: &Tok, term: Term) -> bool {
    match term {
        Term::Eot => *t == Tok::Eot,
        Term::Close => *t == Tok::Close,
    }
}

fn lead_len_of(b: u8) -> usize {
    enc::len(&[b, 0x80, 0x80, 0x80], 0)
}

fn name_end_code(start: u32) -> u32 {
    match u8::try_from(start).unwrap_or(0) {
        b'<' => ch(b'>'),
        b'\'' => ch(b'\''),
        b'(' => ch(b')'),
        _ => 0,
    }
}

/// `conv_backslash_value` (`ONIG_SYN_OP_ESC_CONTROL_CHARS`, no `\v`).
fn conv_backslash_value(c: u32) -> u32 {
    match u8::try_from(c).unwrap_or(0) {
        b'n' => 0x0a,
        b't' => 0x09,
        b'r' => 0x0d,
        b'f' => 0x0c,
        b'a' => 0x07,
        b'b' => 0x08,
        b'e' => 0x1b,
        _ => c,
    }
}

/// The single- or multibyte kind of a class character
/// (`ONIGENC_CODE_TO_MBCLEN`).
fn char_type(code: u32) -> Result<CVal, Error> {
    match enc::code_len(code) {
        Some(1) => Ok(CVal::Sb),
        Some(_) => Ok(CVal::Mb),
        None => Err(Error::new(E::InvalidCodePointValue)),
    }
}

/// Puts a pending value into the class.
fn flush_value(cc: &mut Class, code: u32, ty: CVal) -> Result<(), Error> {
    match ty {
        CVal::Sb => {
            if code > 0xff {
                return Err(Error::new(E::InvalidCodePointValue));
            }
            cc.set_bit(code);
        }
        CVal::Mb => cc.add_range(code, code),
        CVal::CProp | CVal::Undef => {}
    }
    Ok(())
}

/// `cc_char_next`.
fn char_next(
    cc: &mut Class,
    from: &mut u32,
    to: u32,
    intype: CVal,
    ty: &mut CVal,
    state: &mut CState,
) -> Result<(), Error> {
    match *state {
        CState::Value => flush_value(cc, *from, *ty)?,
        CState::Range => {
            if intype == *ty {
                if intype == CVal::Sb {
                    if *from > 0xff || to > 0xff {
                        return Err(Error::new(E::InvalidCodePointValue));
                    }
                    if *from > to {
                        return Err(Error::new(E::EmptyRangeInCharClass));
                    }
                    cc.set_bits(*from, to);
                } else {
                    if *from > to {
                        return Err(Error::new(E::EmptyRangeInCharClass));
                    }
                    cc.add_range(*from, to);
                }
            } else {
                if *from > to {
                    return Err(Error::new(E::EmptyRangeInCharClass));
                }
                cc.set_bits(*from, to.min(0xff));
                cc.add_range(*from, to);
            }
            *state = CState::Complete;
        }
        CState::Complete | CState::Start => *state = CState::Value,
    }
    *from = to;
    *ty = intype;
    Ok(())
}

/// `cc_cprop_next`.
fn cprop_next(
    cc: &mut Class,
    code: &mut u32,
    ty: &mut CVal,
    state: &mut CState,
) -> Result<(), Error> {
    if *state == CState::Range {
        return Err(Error::new(E::CharClassValueAtEndOfRange));
    }
    if *state == CState::Value {
        flush_value(cc, *code, *ty)?;
    }
    *state = CState::Value;
    *ty = CVal::CProp;
    Ok(())
}

/// `add_ctype_to_cc` for UTF-8, with no ASCII-range option:
/// `add_ctype_to_cc_by_range` with `sb_out` 0x80.
pub(crate) fn add_ctype_to_cc(cc: &mut Class, ctype: usize, not: bool) {
    const SB_OUT: u32 = 0x80;
    let mbr = unicode::ranges(ctype);
    if not {
        add_ctype_not(cc, mbr, SB_OUT);
        return;
    }
    {
        for &(from, to) in mbr {
            if to < SB_OUT {
                cc.set_bits(from, to);
            } else if from < SB_OUT {
                cc.set_bits(from, SB_OUT - 1);
                cc.add_range(SB_OUT, to);
            } else {
                cc.add_range(from, to);
            }
        }
    }
}

/// The negated half of `add_ctype_to_cc_by_range`: below `sb_out` the
/// bitset gets what the ranges miss, above it the ranges' complement.
fn add_ctype_not(cc: &mut Class, mbr: &[(u32, u32)], sb_out: u32) {
    let mut prev = 0u32;
    for &(from, to) in mbr {
        for j in prev..from.min(sb_out) {
            cc.set_bit(j);
        }
        prev = to.saturating_add(1);
        if prev >= sb_out {
            break;
        }
    }
    if prev < sb_out {
        cc.set_bits(prev, sb_out - 1);
    }
    let mut prev = sb_out;
    for &(from, to) in mbr {
        if prev < from {
            cc.add_range(prev, from - 1);
        }
        prev = to.wrapping_add(1);
        if prev == 0 {
            return;
        }
    }
    cc.add_range(prev, u32::MAX);
}

/// `i_apply_case_fold` over every fold (`ONIGENC_APPLY_ALL_CASE_FOLD`),
/// with `CASE_FOLD_IS_APPLIED_INSIDE_NEGATIVE_CCLASS`: the class before its
/// negation gains every code point that folds with one it holds, and a
/// class that is not negated also matches the multi-character folds of
/// what it holds.
fn apply_case_fold(mut cc: Class) -> Node {
    let mut add = Vec::new();
    for g in unicode::all_sibling_groups() {
        if g.iter().any(|&x| cc.holds_code(x)) {
            add.extend_from_slice(g);
        }
    }
    for c in add {
        cc.add_code(c);
    }
    let mut alts = Vec::new();
    if !cc.not {
        for (c, to) in unicode::multi_folds() {
            if cc.holds_code(c) {
                let mut seq: Vec<Node> = Vec::new();
                for &x in to {
                    if let Some(unfolds) = unicode::unfolds1(x) {
                        let mut k = Class::default();
                        for &m in unfolds {
                            k.add_code(m);
                        }
                        k.add_code(x);
                        seq.push(Node::Class(Box::new(k)));
                    } else {
                        // `ND_STRING_SET_CASE_EXPANDED`: matched exactly.
                        let mut b = Vec::new();
                        enc::push_code(x, &mut b);
                        match seq.last_mut() {
                            Some(Node::Str { bytes, .. }) => bytes.extend(b),
                            _ => seq.push(Node::Str {
                                bytes: b,
                                icase: false,
                            }),
                        }
                    }
                }
                alts.push(if seq.len() == 1 {
                    seq.remove(0)
                } else {
                    Node::List(seq)
                });
            }
        }
    }
    if alts.is_empty() {
        Node::Class(Box::new(cc))
    } else {
        let mut all = vec![Node::Class(Box::new(cc))];
        all.extend(alts);
        Node::Alt(all)
    }
}

/// `\R` (`node_new_general_newline`): `\r\n`, else one of
/// `[\n\v\f\r\x85  ]`, the choice atomic.
fn general_newline() -> Node {
    let mut cc = Class::default();
    cc.set_bits(0x0a, 0x0d);
    cc.add_range(0x85, 0x85);
    cc.add_range(0x2028, 0x2029);
    Node::IfElse {
        cond: Box::new(Node::Str {
            bytes: b"\r\n".to_vec(),
            icase: false,
        }),
        then: None,
        els: Some(Box::new(Node::Class(Box::new(cc)))),
    }
}

/// `\X` (`make_text_segment`): `(?>\O(?:\Y\O)*)`.
fn text_segment() -> Node {
    Node::Atomic(Box::new(Node::List(vec![
        Node::Any { multiline: true },
        Node::Quant {
            body: Box::new(Node::List(vec![
                Node::Anchor(Anchor::NoTextSegmentBoundary),
                Node::Any { multiline: true },
            ])),
            lower: 0,
            upper: None,
            greedy: true,
        },
    ])))
}

/// Where `\E` ends a `\Q` quote: the quote's end and the text after `\E`.
fn find_quote_end(s: &[u8], from: usize) -> Option<(usize, usize)> {
    let mut p = from;
    while p < s.len() {
        let q = p + enc::len(s, p);
        if s[p] == b'\\' && q < s.len() && s[q] == b'E' {
            return Some((p, q + 1));
        }
        p = q;
    }
    None
}

/// `is_invalid_quantifier_target`.
///
/// Lookaround is an anchor in Oniguruma, `\K` a gimmick. A list is never
/// invalid: Oniguruma's loop over it returns 0 on both paths.
fn is_invalid_quantifier_target(n: &Node) -> bool {
    match n {
        Node::Anchor(_) | Node::Look { .. } | Node::Keep => true,
        Node::Alt(xs) => xs.iter().any(is_invalid_quantifier_target),
        _ => false,
    }
}

/// Joins two nodes into one list.
fn concat(a: Node, b: Node) -> Node {
    let mut items = match a {
        Node::List(xs) => xs,
        n => vec![n],
    };
    match b {
        Node::List(xs) => items.extend(xs),
        n => items.push(n),
    }
    Node::List(items)
}

/// `assign_quantifier_body`: the quantified node, and the head of a string
/// whose last character alone is repeated (`abc+` is `ab(?:c)+`).
fn assign_quantifier_body(
    rep: Rep,
    target: Node,
    group: u8,
) -> Result<(Node, Option<Node>), Error> {
    let wrap = |body: Node| {
        let q = if rep.lower == 1 && rep.upper == Some(1) {
            body
        } else {
            Node::Quant {
                body: Box::new(body),
                lower: rep.lower,
                upper: rep.upper,
                greedy: rep.greedy,
            }
        };
        if rep.possessive {
            Node::Atomic(Box::new(q))
        } else {
            q
        }
    };
    if rep.lower == 1 && rep.upper == Some(1) {
        let q = if rep.possessive {
            Node::Atomic(Box::new(target))
        } else {
            target
        };
        return Ok((q, None));
    }
    if let Node::Str { bytes, icase } = &target
        && group == 0
        && !bytes.is_empty()
        && enc::len(bytes, 0) < bytes.len()
    {
        let last = enc::left_adjust(bytes, 0, bytes.len() - 1);
        let head = Node::Str {
            bytes: bytes[..last].to_vec(),
            icase: *icase,
        };
        let tail = Node::Str {
            bytes: bytes[last..].to_vec(),
            icase: *icase,
        };
        return Ok((wrap(tail), Some(head)));
    }
    if let Node::Quant {
        lower,
        upper,
        greedy,
        ..
    } = &target
    {
        let target_num = quantifier_type(*lower, *upper, *greedy);
        let nest_num = quantifier_type(rep.lower, rep.upper, rep.greedy);
        if target_num.is_some() && nest_num.is_none() {
            // `(?:a*){n,m}`, `(?:a+){n,m}` => `(?:a*){n,n}`, `(?:a+){n,n}`
            let mut rep = rep;
            if matches!(target_num, Some(1 | 2)) && rep.upper.is_some_and(|u| u > 1) && rep.greedy {
                rep.upper = Some(if rep.lower == 0 { 1 } else { rep.lower });
            }
            return Ok((quantified(rep, target), None));
        }
        return Ok((reduce_nested_quantifier(rep, target)?, None));
    }
    Ok((wrap(target), None))
}

/// A quantifier over `body`, possessive as `rep` asks.
fn quantified(rep: Rep, body: Node) -> Node {
    let q = Node::Quant {
        body: Box::new(body),
        lower: rep.lower,
        upper: rep.upper,
        greedy: rep.greedy,
    };
    if rep.possessive {
        Node::Atomic(Box::new(q))
    } else {
        q
    }
}

/// `quantifier_type_num`: `?` 0, `*` 1, `+` 2, `??` 3, `*?` 4, `+?` 5.
fn quantifier_type(lower: u32, upper: Option<u32>, greedy: bool) -> Option<usize> {
    let base = match (lower, upper) {
        (0, Some(1)) => 0,
        (0, None) => 1,
        (1, None) => 2,
        _ => return None,
    };
    Some(if greedy { base } else { base + 3 })
}

/// `onig_reduce_nested_quantifier`: the quantifier `rep` over the
/// quantifier `child`, as one where Oniguruma's table says so
/// (`ReduceTypeTable`, indexed by the child's kind and the parent's).
fn reduce_nested_quantifier(rep: Rep, child: Node) -> Result<Node, Error> {
    use Reduce::{A, Aq, Asis, Del, P, PQq, Qq};
    const TABLE: [[Reduce; 6]; 6] = [
        [Del, A, A, Qq, Aq, Asis],
        [Del, Del, Del, PQq, PQq, Del],
        [A, A, Del, Asis, PQq, Del],
        [Del, Aq, Aq, Del, Aq, Aq],
        [Del, Del, Del, Del, Del, Del],
        [Asis, A, P, Aq, Aq, Del],
    ];
    let Node::Quant {
        body,
        lower,
        upper,
        greedy,
    } = child
    else {
        unreachable!("a nested quantifier")
    };
    let c = (lower, upper, greedy);
    let pnum = quantifier_type(rep.lower, rep.upper, rep.greedy);
    let cnum = quantifier_type(lower, upper, greedy);
    let with = |lower, upper, greedy, body| {
        quantified(
            Rep {
                lower,
                upper,
                greedy,
                ..rep
            },
            body,
        )
    };
    let child = |body| Node::Quant {
        body,
        lower: c.0,
        upper: c.1,
        greedy: c.2,
    };
    let (Some(pnum), Some(cnum)) = (pnum, cnum) else {
        if rep.upper == Some(rep.lower) && c.1 == Some(c.0) {
            // `onig_positive_int_multiply`: past `INT_MAX` is too big.
            let n = rep.lower.checked_mul(c.0).filter(|&n| n <= 0x7fff_ffff);
            let Some(n) = n else {
                return Err(Error::new(E::TooBigNumberForRepeatRange));
            };
            return Ok(with(n, Some(n), rep.greedy, *body));
        }
        return Ok(quantified(rep, child(body)));
    };
    Ok(match TABLE[cnum][pnum] {
        // The parent goes: the child stands as it was, never possessive.
        Del => {
            let n = child(body);
            if rep.possessive {
                Node::Atomic(Box::new(n))
            } else {
                n
            }
        }
        A => with(0, None, true, *body),
        P => with(1, None, true, *body),
        Aq => with(0, None, false, *body),
        Qq => with(0, Some(1), false, *body),
        PQq => with(
            0,
            Some(1),
            false,
            Node::Quant {
                body,
                lower: 1,
                upper: None,
                greedy: true,
            },
        ),
        Asis => quantified(rep, child(body)),
    })
}

/// `enum ReduceType`.
#[derive(Clone, Copy)]
enum Reduce {
    Asis,
    Del,
    A,
    P,
    Aq,
    Qq,
    PQq,
}
