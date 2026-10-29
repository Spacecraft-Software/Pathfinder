//! Pathfinder: jq 1.8.1's `f_match` (`src/builtin.c`), on the Oniguruma
//! port in `pathfinder-regex` (PLAN P-071, P-072).
//!
//! jq builds every regex result here: the search loop (`g` searches on
//! from the end of a match, and one byte past an empty one), offsets and
//! lengths counted in code points by jq's own loop, and the capture
//! objects with their keys in jq's order.

use alloc::vec::Vec;
use pathfinder_regex::{Options, Regex};

/// What jq's modifier string asks for: the regex options and `g`.
pub struct Flags {
    pub options: Options,
    pub global: bool,
}

impl Flags {
    /// The modifiers, or `Err` when jq rejects them (`" is not a valid
    /// modifier string"`).
    pub fn new(flags: &str) -> Result<Self, ()> {
        let options = Options::from_flags(flags).map_err(|_bad| ())?;
        Ok(Self {
            options,
            global: flags.contains('g'),
        })
    }
}

/// A capture as jq reports it: `offset` -1 and `string` `null` for a group
/// that did not take part.
pub struct Capture<'a> {
    pub offset: isize,
    pub length: usize,
    /// The bytes matched, `None` for `null`.
    pub string: Option<&'a [u8]>,
    pub name: Option<&'a [u8]>,
    /// Whether jq writes `offset`, `length` before `string` (a capture that
    /// matched text) or `offset`, `string`, `length` (an empty one).
    pub matched_text: bool,
}

/// A match as jq reports it.
pub struct Match<'a> {
    pub offset: usize,
    pub length: usize,
    pub string: &'a [u8],
    pub captures: Vec<Capture<'a>>,
}

/// `jvp_utf8_decode_length`.
fn decode_len(b: u8) -> usize {
    if b & 0x80 == 0 {
        1
    } else if b & 0xe0 == 0xc0 {
        2
    } else if b & 0xf0 == 0xe0 {
        3
    } else {
        4
    }
}

/// jq's counting of code points: its loops step from byte 0, one
/// `jvp_utf8_decode_length` at a time, for every offset and length. jq
/// starts each count from the start of the string, which makes a global
/// match quadratic; the steps are the same from where the last count
/// stopped, so this goes on from there, and from 0 only for an earlier
/// position.
struct Counter<'a> {
    s: &'a [u8],
    /// Steps taken, and the byte they reach.
    k: usize,
    p: usize,
}

impl<'a> Counter<'a> {
    fn new(s: &'a [u8]) -> Self {
        Self { s, k: 0, p: 0 }
    }

    /// The first step at or past `x`, and the byte it is at.
    fn steps_to(&mut self, x: usize) -> (usize, usize) {
        if x < self.p {
            (self.k, self.p) = (0, 0);
        }
        while self.p < x {
            self.p += decode_len(self.s[self.p]);
            self.k += 1;
        }
        (self.k, self.p)
    }

    /// jq's offset of a zero-width position: the characters starting
    /// before it.
    fn offset_of(&mut self, at: usize) -> usize {
        self.steps_to(at).0
    }

    /// jq's offset and length of `beg..end`, by its loop: the offset is
    /// set only where a step lands on `beg`, so a range starting inside a
    /// character has offset 0 and counts its length from 0.
    fn offset_len(&mut self, beg: usize, end: usize) -> (usize, usize) {
        let (kb, pb) = self.steps_to(beg);
        let (ke, _) = self.steps_to(end);
        if pb == beg {
            (kb, ke - kb)
        } else {
            (0, ke)
        }
    }
}

/// Runs `f_match` on `input`; `test` stops at the first match. The error
/// is Oniguruma's message, for jq's `"Regex failure: "`.
pub fn matches<'a>(
    input: &'a [u8],
    re: &'a Regex,
    flags: &Flags,
    test: bool,
) -> Result<Vec<Match<'a>>, pathfinder_regex::Error> {
    let mut names: Vec<Option<&[u8]>> = alloc::vec![None; re.groups() + 1];
    for (name, groups) in re.names() {
        for &g in groups {
            names[g] = Some(name);
        }
    }
    let end = input.len();
    let mut start = 0;
    let mut out = Vec::new();
    let mut count = Counter::new(input);
    while let Some(region) = re.search(input, start)? {
        let (b0, e0) = region[0].expect("the whole match");
        if test {
            out.push(Match {
                offset: 0,
                length: 0,
                string: &[],
                captures: Vec::new(),
            });
            break;
        }
        if b0 == e0 {
            // A zero-width match: every capture reads as empty there.
            let idx = count.offset_of(b0);
            let captures = (1..region.len())
                .map(|i| Capture {
                    offset: if region[i].is_some() {
                        idx as isize
                    } else {
                        -1
                    },
                    length: 0,
                    string: region[i].map(|_| &input[..0]),
                    name: names[i],
                    matched_text: false,
                })
                .collect();
            out.push(Match {
                offset: idx,
                length: 0,
                string: &input[..0],
                captures,
            });
            // "ensure '"qux" | match("(?=u)"; "g")' matches just once"
            start = e0 + 1;
        } else {
            let (idx, len) = count.offset_len(b0, e0);
            let captures = (1..region.len())
                .map(|i| match region[i] {
                    None => Capture {
                        offset: -1,
                        length: 0,
                        string: None,
                        name: names[i],
                        matched_text: false,
                    },
                    Some((b, e)) if b == e => Capture {
                        offset: count.offset_of(b) as isize,
                        length: 0,
                        string: Some(&input[..0]),
                        name: names[i],
                        matched_text: false,
                    },
                    Some((b, e)) => {
                        let (idx, len) = count.offset_len(b, e);
                        Capture {
                            offset: idx as isize,
                            length: len,
                            string: Some(&input[b..e]),
                            name: names[i],
                            matched_text: true,
                        }
                    }
                })
                .collect();
            out.push(Match {
                offset: idx,
                length: len,
                string: &input[b0..e0],
                captures,
            });
            start = e0;
        }
        if !(flags.global && start <= end) {
            break;
        }
    }
    Ok(out)
}
