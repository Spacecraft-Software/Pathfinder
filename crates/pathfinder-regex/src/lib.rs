// SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
// SPDX-License-Identifier: GPL-3.0-or-later OR MIT

//! Regular expressions that match as jq 1.8.1's do (PLAN P-071, P-072).
//!
//! jq's release binary matches with the Oniguruma 6.9.10 it vendors,
//! compiling every pattern with `ONIG_SYNTAX_PERL_NG`, UTF-8 and
//! `ONIG_OPTION_CAPTURE_GROUP` (`f_match` in jq's `src/builtin.c`). This
//! crate is that one configuration of Oniguruma in Rust: the parser
//! transcribed ([`parse`]), the tree tuned as Oniguruma tunes it
//! ([`tune`]), compiled to the same shapes ([`compile`]) and run by a
//! matcher with Oniguruma's stack discipline ([`exec`]), over Oniguruma's
//! own Unicode tables ([`tables`]).
//!
//! ```
//! use pathfinder_regex::{Options, Regex};
//!
//! let re = Regex::new(br"(?<word>\w+)", Options::default()).unwrap();
//! let m = re.search("αβ γ".as_bytes(), 0).unwrap().unwrap();
//! assert_eq!(m[0], Some((0, 4)));
//! assert_eq!(re.names().next(), Some((&b"word"[..], &[1][..])));
//! ```

mod ast;
mod compile;
mod enc;
mod error;
mod exec;
mod parse;
mod tables;
mod tune;
mod unicode;

pub use error::{Error, ErrorKind};
pub use exec::Region;

/// jq's modifier letters, as Oniguruma options (`f_match`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Options {
    /// `i`: `ONIG_OPTION_IGNORECASE`.
    pub ignorecase: bool,
    /// `x`: `ONIG_OPTION_EXTEND`.
    pub extend: bool,
    /// `m`: `ONIG_OPTION_MULTILINE`, under which `.` matches a newline.
    pub multiline: bool,
    /// `l`: `ONIG_OPTION_FIND_LONGEST`.
    pub longest: bool,
    /// `n`: `ONIG_OPTION_FIND_NOT_EMPTY`.
    pub not_empty: bool,
}

impl Options {
    /// The options of jq's modifier string; `Err` with the first letter
    /// jq does not know. `s` is `Perl_NG`'s default already, `p` is `m` and
    /// `s`, `g` is jq's own and left to the caller.
    pub fn from_flags(flags: &str) -> Result<Self, char> {
        let mut o = Self::default();
        for c in flags.chars() {
            match c {
                'g' | 's' => {}
                'i' => o.ignorecase = true,
                'x' => o.extend = true,
                'm' | 'p' => o.multiline = true,
                'l' => o.longest = true,
                'n' => o.not_empty = true,
                other => return Err(other),
            }
        }
        Ok(o)
    }
}

/// A compiled pattern.
#[derive(Clone, Debug)]
pub struct Regex {
    program: compile::Program,
    groups: usize,
    names: Vec<(Vec<u8>, Vec<usize>)>,
    search: exec::SearchOptions,
}

impl Regex {
    /// Compiles `pattern` as jq does (`onig_new`).
    ///
    /// # Errors
    ///
    /// The pattern is one Oniguruma rejects; the error is worded as
    /// Oniguruma words it.
    pub fn new(pattern: &[u8], options: Options) -> Result<Self, Error> {
        let parsed = parse::parse(
            pattern,
            parse::Options {
                ignorecase: options.ignorecase,
                extend: options.extend,
                multiline: options.multiline,
                // `ONIG_SYNTAX_PERL_NG`'s own option.
                singleline: true,
            },
        )?;
        Ok(Self {
            program: compile::compile(&parsed.root),
            groups: parsed.groups,
            names: parsed.names,
            search: exec::SearchOptions {
                longest: options.longest,
                not_empty: options.not_empty,
            },
        })
    }

    /// The first match from `start` on (`onig_search` with the range to
    /// the end): the match and each group as byte ranges of `text`. A
    /// `start` past the end has no match.
    ///
    /// # Errors
    ///
    /// The match gave up (`retry-limit-in-match over`).
    pub fn search(&self, text: &[u8], start: usize) -> Result<Option<Region>, Error> {
        self.searcher(text).search(start)
    }

    /// Searches of one text, from starts that go forward, as a global
    /// match makes them: each part of the text is scanned for what a
    /// match must contain only once, where a fresh [`Regex::search`] each
    /// time would scan the rest of the text again.
    #[must_use]
    pub fn searcher<'a>(&'a self, text: &'a [u8]) -> Searcher<'a> {
        Searcher {
            re: self,
            text,
            hints: exec::Hints::default(),
        }
    }

    /// The number of capture groups.
    #[must_use]
    pub fn groups(&self) -> usize {
        self.groups
    }

    /// Each group name and the groups it names, in the order the names
    /// were first defined (`onig_foreach_name`).
    pub fn names(&self) -> impl Iterator<Item = (&[u8], &[usize])> {
        self.names.iter().map(|(n, g)| (n.as_slice(), g.as_slice()))
    }
}

/// Searches of one text by one [`Regex`] ([`Regex::searcher`]).
#[derive(Debug)]
pub struct Searcher<'a> {
    re: &'a Regex,
    text: &'a [u8],
    hints: exec::Hints,
}

impl Searcher<'_> {
    /// [`Regex::search`] of this text from `start`.
    ///
    /// # Errors
    ///
    /// The match gave up (`retry-limit-in-match over`).
    pub fn search(&mut self, start: usize) -> Result<Option<Region>, Error> {
        let re = self.re;
        exec::search(
            &re.program,
            re.groups,
            self.text,
            start,
            re.search,
            &mut self.hints,
        )
    }
}
