// SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Fuzz the regex engine (`pathfinder-regex`): any pattern, any text.
//!
//! The input is `pattern NUL text NUL flags`. The pattern's bytes go to the
//! parser as they come, non-UTF-8 included; the text is searched as jq's
//! `g` searches it, on from the end of each match and one byte past an
//! empty one, so starts inside a character are reached too.

#![no_main]

use libfuzzer_sys::fuzz_target;
use pathfinder_regex::{Options, Regex};

fuzz_target!(|data: &[u8]| {
    let mut parts = data.splitn(3, |&b| b == 0);
    let pattern = parts.next().unwrap_or_default();
    let text = String::from_utf8_lossy(parts.next().unwrap_or_default());
    let flags = String::from_utf8_lossy(parts.next().unwrap_or_default());
    let Ok(options) = Options::from_flags(&flags) else {
        return;
    };
    let Ok(re) = Regex::new(pattern, options) else {
        return;
    };
    let text = text.as_bytes();
    let mut start = 0;
    while start <= text.len() {
        match re.search(text, start) {
            Ok(Some(region)) => {
                let (b, e) = region[0].expect("a whole match");
                assert!(b <= e && e <= text.len());
                start = if b == e { e + 1 } else { e };
            }
            _ => break,
        }
    }
});
