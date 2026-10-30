// SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
// SPDX-License-Identifier: GPL-3.0-or-later OR MIT

//! UTF-8 as Oniguruma's `ONIG_ENCODING_UTF8` reads it (`src/utf8.c`).
//!
//! Oniguruma works on byte positions and never checks that one is a
//! character boundary: jq resumes a search one byte past an empty match, so
//! a position can be inside a character. There a continuation byte is a
//! character of its own, one byte long, whose code is the byte's value.

/// Bytes in the character a lead byte starts (`EncLen_UTF8`, with
/// `USE_RFC3629_RANGE`, as `utf8.c` defines it).
const fn lead_len(b: u8) -> usize {
    match b {
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => 1,
    }
}

/// The length of the character at `at`, cut short by the end of `s`
/// (`enclen`, bounded as `mbc_to_code` bounds it).
pub fn len(s: &[u8], at: usize) -> usize {
    lead_len(s[at]).min(s.len() - at)
}

/// The code of the character at `at` (`mbc_to_code`).
pub fn code(s: &[u8], at: usize) -> u32 {
    let n = len(s, at);
    let c = u32::from(s[at]);
    if n == 1 {
        return c;
    }
    let mut v = c & ((1 << (7 - n)) - 1);
    for &b in &s[at + 1..at + n] {
        v = (v << 6) | (u32::from(b) & 0x3f);
    }
    v
}

/// The start of the character before `at`, not before `start`
/// (`onigenc_get_prev_char_head`), or `None` at `start`.
pub fn prev(s: &[u8], start: usize, at: usize) -> Option<usize> {
    if at <= start {
        return None;
    }
    Some(left_adjust(s, start, at - 1))
}

/// The start of the character holding byte `at` (`left_adjust_char_head`).
pub fn left_adjust(s: &[u8], start: usize, at: usize) -> usize {
    let mut p = at;
    while p > start && (s[p] & 0xc0) == 0x80 {
        p -= 1;
    }
    p
}

/// The bytes UTF-8 writes a code point in (`code_to_mbclen`), or `None`
/// past four (`ONIGERR_INVALID_CODE_POINT_VALUE`).
pub const fn code_len(c: u32) -> Option<usize> {
    match c {
        0..=0x7f => Some(1),
        0x80..=0x7ff => Some(2),
        0x800..=0xffff => Some(3),
        0x1_0000..=0x1f_ffff => Some(4),
        _ => None,
    }
}

/// A code point written as UTF-8 (`code_to_mbc`), into `out`; `false` past
/// four bytes (`ONIGERR_TOO_BIG_WIDE_CHAR_VALUE`).
#[expect(
    clippy::cast_possible_truncation,
    reason = "each byte is masked or shifted below 0x100 first"
)]
pub fn push_code(c: u32, out: &mut Vec<u8>) -> bool {
    let Some(n) = code_len(c) else {
        return false;
    };
    if n == 1 {
        out.push(c as u8);
        return true;
    }
    let lead: u32 = [0, 0, 0xc0, 0xe0, 0xf0][n];
    out.push((lead | (c >> (6 * (n - 1)))) as u8);
    for i in (0..n - 1).rev() {
        out.push((0x80 | ((c >> (6 * i)) & 0x3f)) as u8);
    }
    true
}

/// Whether `s` is a string Oniguruma takes as UTF-8 (`is_valid_mbc_string`):
/// no character starts with a continuation byte or ends early.
pub fn is_valid(s: &[u8]) -> bool {
    let mut i = 0;
    while i < s.len() {
        if (s[i] & 0xc0) == 0x80 {
            return false;
        }
        let n = lead_len(s[i]);
        if i + n > s.len() || s[i + 1..i + n].iter().any(|&c| (c & 0xc0) != 0x80) {
            return false;
        }
        i += n;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::{code, left_adjust, len, prev, push_code};

    #[test]
    fn a_continuation_byte_is_a_character_of_its_own() {
        let s = "éa".as_bytes();
        assert_eq!((len(s, 0), code(s, 0)), (2, 0xe9));
        assert_eq!((len(s, 1), code(s, 1)), (1, 0xa9));
        assert_eq!(left_adjust(s, 0, 1), 0);
        assert_eq!(prev(s, 0, 2), Some(0));
        assert_eq!(prev(s, 0, 0), None);
    }

    #[test]
    fn code_points_round_trip() {
        for c in ['a', 'é', '€', '😀'] {
            let mut out = Vec::new();
            assert!(push_code(c as u32, &mut out));
            assert_eq!(out, c.to_string().as_bytes());
            assert_eq!(code(&out, 0), c as u32);
        }
    }
}
