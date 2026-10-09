// SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Capture-group naming for the `match` repair, precomputed for literal regexes.
//!
//! jaq leaves an unmatched group out of a match's `captures`, after which the
//! list no longer says which entry belongs to which group. The repair in
//! `prelude.rs` therefore renames every unnamed capture group to `__pf_gN`
//! before matching and rebuilds the list in group order afterwards. Finding
//! the groups means scanning the regex text, and the prelude does that in the
//! jq language with a regex of its own — which jaq compiles again on every
//! call: measured on 100,000 `match` calls, 10.5 s with the scan against
//! 4.6 s without it.
//!
//! So the regexes written as string literals in the filter are scanned here,
//! once, and handed to the prelude as a lookup table (`_pf_regex_lit`). A
//! regex computed at run time still takes the jq-language scan.
//!
//! # The two scanners must agree
//!
//! [`name_groups`] is a transcription of the prelude's scan, which tokenises
//! left to right with this regex, matched globally:
//!
//! ```text
//! \\.  |  \[\^?\]?(?:\[:[a-z]+:\]|\\.|[^\]\\])*\]  |  \(\?P?<[A-Za-z_][A-Za-z0-9_]*>  |  \(\?  |  \(
//! ```
//!
//! an escape, a character class, a named-group opener, any other `(?`, and a
//! plain `(` — the only token that opens an unnamed capture group. Characters
//! no alternative matches are skipped. Quantifiers are greedy and backtrack,
//! exactly as the regex engine would.

use std::fmt::Write as _;

/// A regex with every capture group named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Named {
    /// The regex with each unnamed group opener `(` replaced by `(?<__pf_gN>`.
    pub regex: String,
    /// Every capture group in order: its jq-visible name (`None` for an
    /// unnamed group), and the name jaq will report it under.
    pub groups: Vec<(Option<String>, String)>,
}

/// Name the capture groups of `re`, or `None` when it contains no `(`.
///
/// `None` mirrors the prelude, which matches a regex without `(` as written.
pub fn name_groups(re: &str) -> Option<Named> {
    if !re.contains('(') {
        return None;
    }
    let chars: Vec<char> = re.chars().collect();
    let mut regex = String::with_capacity(re.len() + 16);
    let mut groups: Vec<(Option<String>, String)> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let token_end = match chars[i] {
            '\\' => chars.get(i + 1).filter(|c| **c != '\n').map(|_| i + 2),
            '[' => class_end(&chars, i),
            '(' if chars.get(i + 1) == Some(&'?') => {
                if let Some((name, end)) = named_opener(&chars, i) {
                    groups.push((Some(name.clone()), name));
                    regex.extend(&chars[i..end]);
                    i = end;
                    continue;
                }
                Some(i + 2)
            }
            '(' => {
                let synthetic = format!("__pf_g{}", groups.len());
                let _ = write!(regex, "(?<{synthetic}>");
                groups.push((None, synthetic));
                i += 1;
                continue;
            }
            _ => None,
        };
        let end = token_end.unwrap_or(i + 1);
        regex.extend(&chars[i..end]);
        i = end;
    }
    Some(Named { regex, groups })
}

/// `\(\?P?<[A-Za-z_][A-Za-z0-9_]*>` at `i`: the group's name and the end.
fn named_opener(chars: &[char], i: usize) -> Option<(String, usize)> {
    // `P?` is greedy: with the `P` first, then without it.
    [true, false].into_iter().find_map(|with_p| {
        let mut k = i + 2;
        if with_p {
            if chars.get(k) != Some(&'P') {
                return None;
            }
            k += 1;
        }
        if chars.get(k) != Some(&'<') {
            return None;
        }
        k += 1;
        let start = k;
        if !chars
            .get(k)
            .is_some_and(|c| c.is_ascii_alphabetic() || *c == '_')
        {
            return None;
        }
        while chars
            .get(k)
            .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_')
        {
            k += 1;
        }
        (chars.get(k) == Some(&'>')).then(|| (chars[start..k].iter().collect(), k + 1))
    })
}

/// `\[\^?\]?(?:\[:[a-z]+:\]|\\.|[^\]\\])*\]` at `i`: the end of the class.
fn class_end(chars: &[char], i: usize) -> Option<usize> {
    let caret = [chars.get(i + 1) == Some(&'^'), false];
    caret.into_iter().find_map(|take_caret| {
        let k = i + 1 + usize::from(take_caret);
        let bracket = [chars.get(k) == Some(&']'), false];
        bracket
            .into_iter()
            .find_map(|take| class_items(chars, k + usize::from(take)))
    })
}

/// `(?:\[:[a-z]+:\]|\\.|[^\]\\])*\]` at `k`, greedy, with backtracking.
fn class_items(chars: &[char], k: usize) -> Option<usize> {
    let posix = || -> Option<usize> {
        if chars.get(k) != Some(&'[') || chars.get(k + 1) != Some(&':') {
            return None;
        }
        let mut j = k + 2;
        while chars.get(j).is_some_and(char::is_ascii_lowercase) {
            j += 1;
        }
        (j > k + 2 && chars.get(j) == Some(&':') && chars.get(j + 1) == Some(&']')).then_some(j + 2)
    };
    let escape = || {
        (chars.get(k) == Some(&'\\') && chars.get(k + 1).is_some_and(|c| *c != '\n'))
            .then_some(k + 2)
    };
    let plain = || {
        chars
            .get(k)
            .filter(|c| **c != ']' && **c != '\\')
            .map(|_| k + 1)
    };
    for next in [posix(), escape(), plain()].into_iter().flatten() {
        if let Some(end) = class_items(chars, next) {
            return Some(end);
        }
    }
    (chars.get(k) == Some(&']')).then_some(k + 1)
}

/// What the prelude may assume about a literal regex.
///
/// Every field errs on the side of the general (slower, always correct) path:
/// a construct this parser does not recognise makes the regex not `simple`
/// and `nullable`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shape {
    /// No capture group can fail to participate in a match, so jaq's
    /// `captures` list is complete and in order without renaming anything.
    pub simple: bool,
    /// The regex can match the empty string somewhere; assertions (`^`, `\b`,
    /// …) count as matching it.
    pub nullable: bool,
}

/// Decide the [`Shape`] of `re` with a small recursive-descent parser of the
/// syntax jq's and jaq's engines share.
pub fn shape(re: &str) -> Shape {
    let chars: Vec<char> = re.chars().collect();
    let mut p = Shapes {
        c: &chars,
        i: 0,
        optional: Vec::new(),
        ok: true,
    };
    let (nullable, _) = p.alt();
    if p.i != chars.len() {
        p.ok = false;
    }
    Shape {
        simple: p.ok && !p.optional.iter().any(|o| *o),
        nullable: !p.ok || nullable,
    }
}

/// The parser behind [`shape`]. `optional[n]` records whether capture group
/// `n` can be skipped; `ok` drops to `false` on anything unrecognised.
struct Shapes<'a> {
    c: &'a [char],
    i: usize,
    optional: Vec<bool>,
    ok: bool,
}

impl Shapes<'_> {
    fn peek(&self) -> Option<char> {
        self.c.get(self.i).copied()
    }

    /// `concat ('|' concat)*`: nullable, and the capture groups inside.
    fn alt(&mut self) -> (bool, Vec<usize>) {
        let (mut nullable, mut caps) = self.concat();
        let mut branches = 1;
        while self.ok && self.peek() == Some('|') {
            self.i += 1;
            let (n, c) = self.concat();
            nullable |= n;
            caps.extend(c);
            branches += 1;
        }
        if branches > 1 {
            // A group in one branch does not take part when another matches.
            self.skippable(&caps);
        }
        (nullable, caps)
    }

    fn concat(&mut self) -> (bool, Vec<usize>) {
        let mut nullable = true;
        let mut caps = Vec::new();
        while self.ok && !matches!(self.peek(), None | Some('|' | ')')) {
            let (n, c) = self.item();
            nullable &= n;
            caps.extend(c);
        }
        (nullable, caps)
    }

    fn skippable(&mut self, caps: &[usize]) {
        for &n in caps {
            self.optional[n] = true;
        }
    }

    /// An atom and its quantifiers.
    fn item(&mut self) -> (bool, Vec<usize>) {
        let (mut nullable, caps) = self.atom();
        while self.ok {
            let zero = match self.peek() {
                Some('?' | '*') => {
                    self.i += 1;
                    true
                }
                Some('+') => {
                    self.i += 1;
                    false
                }
                Some('{') => {
                    let Some(min) = self.counted() else {
                        self.ok = false;
                        return (true, caps);
                    };
                    min == 0
                }
                _ => break,
            };
            // A lazy or possessive suffix changes nothing here.
            if matches!(self.peek(), Some('?' | '+')) {
                self.i += 1;
            }
            if zero {
                self.skippable(&caps);
                nullable = true;
            }
        }
        (nullable, caps)
    }

    /// `{n}`, `{n,}`, `{n,m}` or `{,m}`: the minimum, or `None`.
    fn counted(&mut self) -> Option<u32> {
        let close = self.c[self.i..].iter().position(|c| *c == '}')? + self.i;
        let body: String = self.c[self.i + 1..close].iter().collect();
        let min = match body.split_once(',') {
            Some((lo, hi)) if hi.chars().all(|c| c.is_ascii_digit()) => {
                if lo.is_empty() {
                    Some(0)
                } else {
                    lo.parse().ok()
                }
            }
            None => body.parse().ok(),
            Some(_) => None,
        }?;
        self.i = close + 1;
        Some(min)
    }

    fn atom(&mut self) -> (bool, Vec<usize>) {
        let Some(c) = self.peek() else {
            self.ok = false;
            return (true, Vec::new());
        };
        match c {
            '(' => self.group(),
            '[' => {
                if let Some(end) = class_end(self.c, self.i) {
                    self.i = end;
                    (false, Vec::new())
                } else {
                    self.ok = false;
                    (true, Vec::new())
                }
            }
            '\\' => self.escape(),
            '^' | '$' => {
                self.i += 1;
                (true, Vec::new())
            }
            _ => {
                self.i += 1;
                (false, Vec::new())
            }
        }
    }

    /// An escape: assertions match empty, anything with a brace body or a
    /// backreference is left to the general path.
    fn escape(&mut self) -> (bool, Vec<usize>) {
        match self.c.get(self.i + 1) {
            Some('b' | 'B' | 'A' | 'z' | 'Z' | 'G') => {
                self.i += 2;
                (true, Vec::new())
            }
            Some('Q' | 'k' | 'g' | 'K' | '1'..='9') | None => {
                self.ok = false;
                (true, Vec::new())
            }
            Some(_) => {
                self.i += 2;
                if self.peek() == Some('{') {
                    let close = self.c[self.i..].iter().position(|c| *c == '}');
                    match close {
                        Some(off) => self.i += off + 1,
                        None => self.ok = false,
                    }
                }
                (false, Vec::new())
            }
        }
    }

    /// A parenthesised group of any kind.
    fn group(&mut self) -> (bool, Vec<usize>) {
        self.i += 1;
        let mut capture = None;
        if self.peek() == Some('?') {
            self.i += 1;
            match self.peek() {
                Some(':') => self.i += 1,
                // A named group, or (failing that) a lookbehind.
                Some('<' | 'P') => {
                    let Some((_, end)) = named_opener(self.c, self.i - 2) else {
                        self.ok = false;
                        return (true, Vec::new());
                    };
                    self.i = end;
                    capture = Some(self.optional.len());
                    self.optional.push(false);
                }
                // Inline flags, `(?i)`, or flags scoped to a group, `(?i:…)`.
                // Extended mode makes whitespace and `#` mean something else.
                Some(f) if f.is_ascii_alphabetic() || f == '-' => {
                    while self
                        .peek()
                        .is_some_and(|c| c.is_ascii_alphabetic() || c == '-')
                    {
                        if self.peek() == Some('x') {
                            self.ok = false;
                        }
                        self.i += 1;
                    }
                    match self.peek() {
                        Some(')') => {
                            self.i += 1;
                            return (true, Vec::new());
                        }
                        Some(':') => self.i += 1,
                        _ => self.ok = false,
                    }
                }
                // Lookaround, comments, and anything else: the general path.
                _ => {
                    self.ok = false;
                    return (true, Vec::new());
                }
            }
        } else {
            capture = Some(self.optional.len());
            self.optional.push(false);
        }
        let (nullable, mut caps) = self.alt();
        if self.peek() == Some(')') {
            self.i += 1;
        } else {
            self.ok = false;
        }
        if let Some(n) = capture {
            caps.insert(0, n);
        }
        (nullable, caps)
    }
}

/// Every literal regex passed to a repaired regex builtin in `src`.
///
/// A literal is a plain string (no interpolation, no `@format`) given as the
/// first argument of `match`, `capture`, `scan`, `sub` or `gsub`, or as the
/// first element of a `[re, flags]` array there; its value is decoded the way
/// jq's lexer decodes it. A filter the parser rejects yields none, and every
/// regex then takes the prelude's run-time path.
pub fn regex_args(src: &str) -> Vec<String> {
    use crate::syntax::{Kind, Node, StrSeg};
    fn plain(n: &Node, src: &str) -> Option<String> {
        match &n.kind {
            Kind::Str(s) if s.format.is_none() => {
                s.parts.iter().try_fold(String::new(), |mut out, p| {
                    match p {
                        StrSeg::Text(span) => out.push_str(&decode(&src[span.start..span.end])?),
                        StrSeg::Interp(_) => return None,
                    }
                    Some(out)
                })
            }
            Kind::Array(Some(inner)) => match &inner.kind {
                Kind::Comma(first, _) => plain(first, src),
                _ => plain(inner, src),
            },
            Kind::Paren(inner) => plain(inner, src),
            _ => None,
        }
    }
    fn walk(n: &Node, src: &str, out: &mut Vec<String>) {
        if let Kind::Call { name, args } = &n.kind
            && matches!(name.as_str(), "match" | "capture" | "scan" | "sub" | "gsub")
            && let Some(re) = args.first().and_then(|a| plain(a, src))
            && !out.contains(&re)
        {
            out.push(re);
        }
        for child in n.children() {
            walk(child, src, out);
        }
    }
    let mut out = Vec::new();
    if let Ok(tree) = crate::syntax::parse::parse(src) {
        walk(&tree, src, &mut out);
    }
    out
}

/// Decode a jq string literal's escapes, or `None` for one jq would reject.
fn decode(raw: &str) -> Option<String> {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next()? {
            '"' => out.push('"'),
            '\\' => out.push('\\'),
            '/' => out.push('/'),
            'b' => out.push('\u{8}'),
            'f' => out.push('\u{c}'),
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            'u' => {
                let hex = |it: &mut std::str::Chars<'_>| -> Option<u32> {
                    let s: String = it.take(4).collect();
                    (s.len() == 4).then(|| u32::from_str_radix(&s, 16).ok())?
                };
                let hi = hex(&mut chars)?;
                let cp = if (0xD800..0xDC00).contains(&hi) {
                    // A surrogate pair, as jq's lexer combines them.
                    let rest = chars.as_str();
                    let lo = rest
                        .strip_prefix("\\u")
                        .and_then(|r| u32::from_str_radix(r.get(..4)?, 16).ok())
                        .filter(|lo| (0xDC00..0xE000).contains(lo))?;
                    chars = rest[6..].chars();
                    0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
                } else {
                    hi
                };
                out.push(char::from_u32(cp)?);
            }
            _ => return None,
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::{Named, Shape, decode, name_groups, regex_args, shape};

    fn named(re: &str) -> (String, Vec<Option<String>>) {
        let Named { regex, groups } = name_groups(re).expect("has a group");
        (regex, groups.into_iter().map(|(name, _)| name).collect())
    }

    #[test]
    fn a_regex_without_parentheses_is_left_alone() {
        assert_eq!(name_groups("a+b"), None);
    }

    #[test]
    fn unnamed_groups_get_synthetic_names_in_order() {
        let (re, names) = named("(a)(?<x>b)(c)");
        assert_eq!(re, "(?<__pf_g0>a)(?<x>b)(?<__pf_g2>c)");
        assert_eq!(names, [None, Some("x".to_owned()), None]);
    }

    #[test]
    fn non_capturing_forms_and_escapes_are_skipped() {
        let (re, names) = named(r"(?:a)(?i:b)\(c(d)(?=e)");
        assert_eq!(re, r"(?:a)(?i:b)\(c(?<__pf_g0>d)(?=e)");
        assert_eq!(names, [None]);
    }

    #[test]
    fn parentheses_inside_classes_are_not_groups() {
        let (re, _) = named("[(]x[^)(]+(y)[]a(]");
        assert_eq!(re, "[(]x[^)(]+(?<__pf_g0>y)[]a(]");
        let (re, _) = named("[[:alpha:](](z)");
        assert_eq!(re, "[[:alpha:](](?<__pf_g0>z)");
    }

    #[test]
    fn an_unclosed_class_is_not_a_token() {
        // `[` with no `]` matches nothing, so the scan steps past it.
        let (re, _) = named("[(a)");
        assert_eq!(re, "[(?<__pf_g0>a)");
    }

    #[test]
    fn python_style_names_are_named_groups() {
        let (re, names) = named("(?P<year>[0-9]+)-(m)");
        assert_eq!(re, "(?P<year>[0-9]+)-(?<__pf_g1>m)");
        assert_eq!(names, [Some("year".to_owned()), None]);
    }

    #[test]
    fn regex_arguments_are_decoded_and_interpolated_ones_skipped() {
        let found = regex_args(
            r#"match("a\\(b)(c)") | "\(capture(["(?<x>y)", "g"]))" | test("(t)") | sub("\(.)"; "z")"#,
        );
        assert_eq!(found, [r"a\(b)(c)", "(?<x>y)"]);
    }

    fn simple(re: &str) -> bool {
        shape(re).simple
    }

    fn nullable(re: &str) -> bool {
        shape(re).nullable
    }

    #[test]
    fn groups_that_always_take_part_are_simple() {
        assert!(simple("id=([0-9]+)"));
        assert!(simple("(?<u>[a-z]+)(?<n>[0-9]+)@"));
        assert!(simple("(a|b)(c)"));
        assert!(simple("(?:x(y))+"));
        assert!(simple("no groups at all"));
    }

    #[test]
    fn groups_that_can_be_skipped_are_not() {
        assert!(!simple("(a)?b"));
        assert!(!simple("(?:(a))*"));
        assert!(!simple("(a)|(b)"));
        assert!(!simple("x(a){0,2}"));
        assert!(!simple("x(?:(a)|b)"));
        // Unrecognised constructs fall back to the general path.
        assert!(!simple("(a)(?=b)"));
        assert!(!simple("(?x) (a) # (b)"));
    }

    #[test]
    fn nullability() {
        assert_eq!(
            shape("[a-z]*"),
            Shape {
                simple: true,
                nullable: true
            }
        );
        assert!(nullable("a?"));
        assert!(nullable("\\b"));
        assert!(nullable("^"));
        assert!(nullable("a|"));
        assert!(!nullable("[0-9]"));
        assert!(!nullable("\\s*,\\s*"));
        assert!(!nullable("a+"));
        assert!(!nullable("(?i)abc"));
    }

    #[test]
    fn decode_follows_jqs_escapes() {
        assert_eq!(decode(r"é\n\/").as_deref(), Some("é\n/"));
        assert_eq!(decode(r"😀").as_deref(), Some("😀"));
        assert_eq!(decode(r"\q"), None);
    }
}
