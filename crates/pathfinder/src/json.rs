// SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A minimal JSON writer for the native verbs' machine-readable reports.
//!
//! The documents are small, flat and built once per process, so a string
//! builder is all they need. Pulling in `serde_json` for them would add the
//! jq path's first runtime dependency, which `Cargo.toml` rules out: the
//! common path is parse-argv-then-execve and stays std-only.

use std::fmt::Write as _;

/// Append `s` to `out` as a JSON string literal, escaped per RFC 8259 §7.
///
/// The quote, the backslash and every control character below U+0020 are
/// escaped — the common ones by their short forms, the rest as `\u00XX`.
/// Everything else, non-ASCII included, is valid in a JSON string as-is.
pub fn push_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if c < ' ' => {
                // Writing into a `String` cannot fail.
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// A JSON object under construction. Members are written in insertion order.
#[derive(Debug)]
pub struct Object {
    buf: String,
}

impl Object {
    /// Start an empty object.
    pub fn new() -> Self {
        Self {
            buf: String::from("{"),
        }
    }

    /// Write the separator and the key for the next member.
    fn key(&mut self, key: &str) {
        if self.buf.len() > 1 {
            self.buf.push(',');
        }
        push_str(&mut self.buf, key);
        self.buf.push(':');
    }

    /// Add a string member.
    pub fn str(mut self, key: &str, value: &str) -> Self {
        self.key(key);
        push_str(&mut self.buf, value);
        self
    }

    /// Add a boolean member.
    pub fn bool(mut self, key: &str, value: bool) -> Self {
        self.key(key);
        self.buf.push_str(if value { "true" } else { "false" });
        self
    }

    /// Add an integer member.
    pub fn int(mut self, key: &str, value: i64) -> Self {
        self.key(key);
        // Writing into a `String` cannot fail.
        let _ = write!(self.buf, "{value}");
        self
    }

    /// Add an array-of-strings member.
    pub fn strs<S: AsRef<str>>(mut self, key: &str, values: &[S]) -> Self {
        self.key(key);
        self.buf.push('[');
        for (i, v) in values.iter().enumerate() {
            if i > 0 {
                self.buf.push(',');
            }
            push_str(&mut self.buf, v.as_ref());
        }
        self.buf.push(']');
        self
    }

    /// Add a nested object member.
    pub fn obj(mut self, key: &str, value: Self) -> Self {
        self.key(key);
        self.buf.push_str(&value.finish());
        self
    }

    /// Add a string member only when `value` is present.
    pub fn opt_str(self, key: &str, value: Option<&str>) -> Self {
        match value {
            Some(v) => self.str(key, v),
            None => self,
        }
    }

    /// Add an array-of-strings member only when it is non-empty.
    pub fn nonempty_strs<S: AsRef<str>>(self, key: &str, values: &[S]) -> Self {
        if values.is_empty() {
            self
        } else {
            self.strs(key, values)
        }
    }

    /// Close the object and return its text.
    pub fn finish(mut self) -> String {
        self.buf.push('}');
        self.buf
    }
}

#[cfg(test)]
mod tests {
    use super::{Object, push_str};

    fn quoted(s: &str) -> String {
        let mut out = String::new();
        push_str(&mut out, s);
        out
    }

    #[test]
    fn strings_escape_what_rfc_8259_requires() {
        assert_eq!(quoted("plain"), r#""plain""#);
        assert_eq!(quoted(r#"a"b\c"#), r#""a\"b\\c""#);
        assert_eq!(quoted("\n\r\t\u{08}\u{0c}"), r#""\n\r\t\b\f""#);
        assert_eq!(quoted("\u{00}\u{01}\u{1f}"), r#""\u0000\u0001\u001f""#);
        // DEL and non-ASCII are legal unescaped.
        assert_eq!(quoted("\u{7f}é→"), "\"\u{7f}é→\"");
        // `/` may be escaped but need not be.
        assert_eq!(quoted("a/b"), r#""a/b""#);
    }

    #[test]
    fn every_escape_round_trips_through_a_real_parser() {
        let all: String = (0u8..0x80).map(char::from).collect::<String>() + "é→😀";
        let parsed: String = serde_json::from_str(&quoted(&all)).expect("valid JSON");
        assert_eq!(parsed, all);
    }

    #[test]
    fn objects_keep_insertion_order_and_omit_nothing_unasked() {
        let inner = Object::new().bool("t", true).int("n", -3);
        let text = Object::new()
            .str("s", "x")
            .strs("a", &["p", "q"])
            .strs("e", &[] as &[&str])
            .obj("o", inner)
            .finish();
        assert_eq!(
            text,
            r#"{"s":"x","a":["p","q"],"e":[],"o":{"t":true,"n":-3}}"#
        );
        assert_eq!(Object::new().finish(), "{}");
    }

    #[test]
    fn optional_members_are_omitted_rather_than_null() {
        let text = Object::new()
            .opt_str("a", None)
            .nonempty_strs("b", &[] as &[&str])
            .opt_str("c", Some("y"))
            .nonempty_strs("d", &["z"])
            .finish();
        assert_eq!(text, r#"{"c":"y","d":["z"]}"#);
    }
}
