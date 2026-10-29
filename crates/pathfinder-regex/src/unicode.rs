// SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
// SPDX-License-Identifier: GPL-3.0-or-later OR MIT

//! Unicode as Oniguruma 6.9.10 knows it: character types and properties,
//! case folding and grapheme cluster breaks, all from [`crate::tables`].

use std::sync::OnceLock;

use crate::tables::{
    CODE_RANGES, EGCB, Egcb, FOLD1, FOLD2, FOLD3, ISO_8859_1_CTYPE, PROPERTY_NAME_MAX_SIZE,
    PROPERTY_NAMES,
};

/// Oniguruma's standard character types (`ONIGENC_CTYPE_*`), which are also
/// the first indices of [`CODE_RANGES`].
pub mod ctype {
    pub const ALPHA: usize = 1;
    pub const BLANK: usize = 2;
    pub const CNTRL: usize = 3;
    pub const DIGIT: usize = 4;
    pub const GRAPH: usize = 5;
    pub const LOWER: usize = 6;
    pub const PRINT: usize = 7;
    pub const PUNCT: usize = 8;
    pub const SPACE: usize = 9;
    pub const UPPER: usize = 10;
    pub const XDIGIT: usize = 11;
    pub const WORD: usize = 12;
    pub const ALNUM: usize = 13;
    pub const ASCII: usize = 14;
}

/// The code ranges of a character type or property, sorted.
pub fn ranges(ctype: usize) -> &'static [(u32, u32)] {
    CODE_RANGES[ctype]
}

/// Whether `c` has the character type or property `ctype`
/// (`onigenc_unicode_is_code_ctype`): below 256 a standard type is read
/// from Oniguruma's ISO-8859-1 table, which differs from the ranges a
/// class is built from (`\w` matches `½`, `[\w]` does not).
pub fn is_ctype(c: u32, ctype: usize) -> bool {
    if ctype <= ctype::ASCII
        && let Ok(b) = u8::try_from(c)
    {
        return ISO_8859_1_CTYPE[usize::from(b)] & (1 << ctype) != 0;
    }
    in_ranges(CODE_RANGES[ctype], c)
}

/// Whether `c` is in sorted, disjoint `ranges`.
pub fn in_ranges(ranges: &[(u32, u32)], c: u32) -> bool {
    let i = ranges.partition_point(|&(_, hi)| hi < c);
    i < ranges.len() && ranges[i].0 <= c
}

/// Whether `c` is a word character (`ONIGENC_IS_CODE_WORD`).
pub fn is_word(c: u32) -> bool {
    is_ctype(c, ctype::WORD)
}

/// The ctype of a property name (`onigenc_unicode_property_name_to_ctype`):
/// ASCII only, `' '`, `'-'` and `'_'` ignored, case ignored.
pub fn property(name: &[u8]) -> Option<usize> {
    let mut buf = Vec::with_capacity(name.len());
    let mut i = 0;
    while i < name.len() {
        let c = crate::enc::code(name, i);
        if c >= 0x80 {
            return None;
        }
        if !matches!(c, 0x20 | 0x2d | 0x5f) {
            buf.push(u8::try_from(c).ok()?.to_ascii_lowercase());
            if buf.len() >= PROPERTY_NAME_MAX_SIZE {
                return None;
            }
        }
        i += crate::enc::len(name, i);
    }
    let key = std::str::from_utf8(&buf).ok()?;
    PROPERTY_NAMES
        .binary_search_by(|(n, _)| (*n).cmp(key))
        .ok()
        .map(|i| usize::from(PROPERTY_NAMES[i].1))
}

/// What a code point folds to: itself, or one to three code points.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fold {
    codes: [u32; 3],
    len: u8,
}

impl Fold {
    pub fn as_slice(&self) -> &[u32] {
        &self.codes[..usize::from(self.len)]
    }
}

/// The case fold of `c`, as Oniguruma folds with `ONIGENC_CASE_FOLD_MIN`
/// (multi-character folds included, the Turkish ones not).
pub fn fold(c: u32) -> Fold {
    if let Ok(i) = FOLD1.binary_search_by_key(&c, |&(k, _)| k) {
        return Fold {
            codes: [FOLD1[i].1, 0, 0],
            len: 1,
        };
    }
    if let Ok(i) = FOLD2.binary_search_by_key(&c, |&(k, _)| k) {
        let [a, b] = FOLD2[i].1;
        return Fold {
            codes: [a, b, 0],
            len: 2,
        };
    }
    if let Ok(i) = FOLD3.binary_search_by_key(&c, |&(k, _)| k) {
        return Fold {
            codes: FOLD3[i].1,
            len: 3,
        };
    }
    Fold {
        codes: [c, 0, 0],
        len: 1,
    }
}

/// The fold tables inverted, as Oniguruma's unfold lookups read them.
struct Unfolds {
    /// Each single fold target and the code points folding to it
    /// (`onigenc_unicode_fold1_key`, `FOLDS1_UNFOLDS`), sorted by target.
    one: Vec<(u32, Vec<u32>)>,
    /// Each multi-character fold and the code points folding to it
    /// (`onigenc_unicode_fold2_key`, `fold3_key`), sorted by fold.
    many: Vec<(Vec<u32>, Vec<u32>)>,
    /// Code points that match one another one for one, as
    /// `ONIGENC_APPLY_ALL_CASE_FOLD` pairs them.
    groups: Vec<Vec<u32>>,
}

fn unfolds() -> &'static Unfolds {
    static UNFOLDS: OnceLock<Unfolds> = OnceLock::new();
    UNFOLDS.get_or_init(|| {
        let mut one: Vec<(u32, Vec<u32>)> = Vec::new();
        let mut by_key: Vec<(u32, u32)> = FOLD1.iter().map(|&(c, k)| (k, c)).collect();
        by_key.sort_unstable();
        for (k, c) in by_key {
            match one.last_mut() {
                Some((last, cs)) if *last == k => cs.push(c),
                _ => one.push((k, vec![c])),
            }
        }
        let mut many: Vec<(Vec<u32>, Vec<u32>)> = Vec::new();
        let mut by_fold: Vec<(Vec<u32>, u32)> =
            multi_folds().map(|(c, f)| (f.to_vec(), c)).collect();
        by_fold.sort_unstable();
        for (f, c) in by_fold {
            match many.last_mut() {
                Some((last, cs)) if *last == f => cs.push(c),
                _ => many.push((f, vec![c])),
            }
        }
        // `apply_case_fold1` pairs a target with its unfolds and the
        // unfolds with one another; `apply_case_fold2`/`3` pair the
        // unfolds of one multi-character fold.
        let mut groups: Vec<Vec<u32>> = one
            .iter()
            .map(|(k, cs)| {
                let mut g = vec![*k];
                g.extend(cs);
                g
            })
            .collect();
        groups.extend(
            many.iter()
                .filter(|(_, cs)| cs.len() > 1)
                .map(|(_, cs)| cs.clone()),
        );
        Unfolds { one, many, groups }
    })
}

/// Every group of code points that match one another one for one.
pub fn all_sibling_groups() -> &'static [Vec<u32>] {
    &unfolds().groups
}

/// The code points folding to `c` alone, if any (`fold1_key`).
pub fn unfolds1(c: u32) -> Option<&'static [u32]> {
    let u = unfolds();
    u.one
        .binary_search_by_key(&c, |(k, _)| *k)
        .ok()
        .map(|i| u.one[i].1.as_slice())
}

/// The code points folding to the sequence `f`, if any (`fold2_key`,
/// `fold3_key`).
pub fn unfolds_n(f: &[u32]) -> Option<&'static [u32]> {
    let u = unfolds();
    u.many
        .binary_search_by(|(k, _)| k.as_slice().cmp(f))
        .ok()
        .map(|i| u.many[i].1.as_slice())
}

/// What `c` folds to when it folds to something else
/// (`onigenc_unicode_unfold_key`), `None` otherwise.
pub fn unfold_key(c: u32) -> Option<Fold> {
    let f = fold(c);
    (f.as_slice() != [c]).then_some(f)
}

/// The code points whose fold is more than one code point, with that fold
/// (`ß` → `ss`).
pub fn multi_folds() -> impl Iterator<Item = (u32, &'static [u32])> {
    FOLD2
        .iter()
        .map(|(c, f)| (*c, f.as_slice()))
        .chain(FOLD3.iter().map(|(c, f)| (*c, f.as_slice())))
}

/// The grapheme cluster break class of `c`, `None` for `Other`.
pub fn egcb(c: u32) -> Option<Egcb> {
    let i = EGCB.partition_point(|&(_, hi, _)| hi < c);
    (i < EGCB.len() && EGCB[i].0 <= c).then(|| EGCB[i].2)
}

#[cfg(test)]
mod tests {
    use super::{ctype, fold, is_ctype, property};

    #[test]
    fn character_types_are_unicodes() {
        assert!(is_ctype('é' as u32, ctype::WORD));
        assert!(is_ctype('٣' as u32, ctype::DIGIT));
        assert!(is_ctype(0xa0, ctype::SPACE));
        assert!(!is_ctype('-' as u32, ctype::WORD));
    }

    #[test]
    fn property_names_ignore_case_spaces_and_separators() {
        assert_eq!(property(b"Greek"), property(b"g r-e_e K"));
        assert!(property(b"L").is_some());
        assert_eq!(property(b"nosuch"), None);
        assert_eq!(property("Gréek".as_bytes()), None);
    }

    #[test]
    fn folds_reach_several_code_points() {
        assert_eq!(fold('A' as u32).as_slice(), ['a' as u32]);
        assert_eq!(fold('ß' as u32).as_slice(), ['s' as u32, 's' as u32]);
        assert_eq!(fold('a' as u32).as_slice(), ['a' as u32]);
        let k = super::unfolds1('k' as u32).expect("k is a fold target");
        assert!(k.contains(&('K' as u32)) && k.contains(&0x212a));
        assert_eq!(
            super::unfolds_n(&['s' as u32, 's' as u32]),
            Some(&[0xdf, 0x1e9e][..])
        );
    }
}
