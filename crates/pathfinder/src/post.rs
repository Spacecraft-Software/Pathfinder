// SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Byte transforms for the two jq flags the engine's driver has no
//! equivalent for.
//!
//! # `-a` / `--ascii-output`
//!
//! Verified against jq 1.8.1: `jq -ra '"café"'` prints `"caf\u00e9"` — **with
//! quotes**. `-a` cancels `-r`'s rawness for strings. That is what makes this
//! transform simple: Pathfinder takes the raw modes off the engine's options,
//! so everything arriving here is JSON, and in JSON a non-ASCII byte can only
//! occur inside a string literal. No parser is needed — escape every
//! non-ASCII scalar and the result is exactly jq's.
//!
//! # `--seq`
//!
//! jq's `--seq` is `application/json-seq` in *both* directions: it emits
//! `RS value LF`, and it also *requires* RS-framed input — feeding jq a plain
//! `1\n2\n` under `--seq` is a parse error. The engine's reader knows nothing
//! about RS, so the input needs its separators stripped on the way in as well
//! as added on the way out.

use std::io::{self, Read, Write};

/// What to write after each value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Terminator {
    /// jq's default: a newline after every value.
    Newline,
    /// `-j` / `--join-output`: nothing between values.
    None,
    /// `--raw-output0`: a NUL after every value.
    Nul,
}

/// The transform to apply to jaq's stdout.
#[derive(Debug, Clone, Copy)]
pub struct Post {
    /// Escape non-ASCII as `\uXXXX`.
    pub ascii: bool,
    /// Prefix each value with RS (0x1E).
    pub seq: bool,
    pub terminator: Terminator,
}

impl Post {
    /// Whether any transform is needed at all.
    ///
    /// When this is false the caller can `exec` and never return, which is the
    /// whole point of keeping this module narrow.
    pub const fn is_identity(self) -> bool {
        !self.ascii && !self.seq && matches!(self.terminator, Terminator::Newline)
    }
}

/// The record separator that frames `application/json-seq`.
const RS: u8 = 0x1E;

/// Splits a stream of top-level JSON values, tracking string and nesting state.
///
/// jaq terminates every top-level value with a newline, and a newline can only
/// appear at nesting depth zero *between* values — inside a pretty-printed
/// container the depth is non-zero, and inside a string a literal newline is
/// escaped. So "newline at depth zero, outside a string" is an exact value
/// boundary rather than a heuristic.
#[derive(Debug, Default)]
struct Splitter {
    depth: i32,
    in_string: bool,
    escaped: bool,
}

impl Splitter {
    /// Feed one byte; returns true when it completes a value.
    fn feed(&mut self, b: u8) -> bool {
        if self.in_string {
            if self.escaped {
                self.escaped = false;
            } else if b == b'\\' {
                self.escaped = true;
            } else if b == b'"' {
                self.in_string = false;
            }
            return false;
        }
        match b {
            b'"' => self.in_string = true,
            b'[' | b'{' => self.depth += 1,
            b']' | b'}' => self.depth -= 1,
            b'\n' if self.depth <= 0 => return true,
            _ => {}
        }
        false
    }
}

/// A writer applying `post` to the values written through it.
///
/// The engine writes its outputs through one. Call [`Framer::finish`] after
/// the last value.
pub struct Framer<W: Write> {
    inner: W,
    post: Post,
    splitter: Splitter,
    value: Vec<u8>,
}

impl<W: Write> Framer<W> {
    pub fn new(inner: W, post: Post) -> Self {
        Self {
            inner,
            post,
            splitter: Splitter::default(),
            value: Vec::with_capacity(4096),
        }
    }

    /// Write a final value that has no trailing newline, and flush.
    pub fn finish(&mut self) -> io::Result<()> {
        if !self.value.is_empty() {
            emit(&mut self.inner, &self.value, self.post)?;
            self.value.clear();
        }
        self.inner.flush()
    }
}

impl<W: Write> Write for Framer<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        for &b in buf {
            if self.splitter.feed(b) {
                emit(&mut self.inner, &self.value, self.post)?;
                self.value.clear();
            } else {
                self.value.push(b);
            }
        }
        Ok(buf.len())
    }

    /// A partial value stays held: only [`Framer::finish`] writes it.
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Write one value with its framing applied.
fn emit(output: &mut impl Write, value: &[u8], post: Post) -> io::Result<()> {
    if post.seq {
        output.write_all(&[RS])?;
    }
    if post.ascii {
        write_ascii(output, value)?;
    } else {
        output.write_all(value)?;
    }
    match post.terminator {
        Terminator::Newline => output.write_all(b"\n"),
        Terminator::None => Ok(()),
        Terminator::Nul => output.write_all(&[0]),
    }
}

/// Write `value`, escaping every non-ASCII scalar the way jq's `-a` does.
///
/// jq emits lowercase hex, and encodes astral-plane characters as a surrogate
/// pair: `"😀"` becomes `"\ud83d\ude00"`. Bytes that are not valid UTF-8 are
/// passed through rather than corrupted — jaq should not emit any, and silently
/// mangling them would be worse than leaving them alone.
fn write_ascii(output: &mut impl Write, value: &[u8]) -> io::Result<()> {
    let mut i = 0usize;
    while i < value.len() {
        let b = value[i];
        if b < 0x80 {
            output.write_all(&[b])?;
            i += 1;
            continue;
        }
        let len = utf8_len(b);
        let Some(chunk) = value.get(i..i + len) else {
            output.write_all(&[b])?;
            i += 1;
            continue;
        };
        if let Ok(s) = std::str::from_utf8(chunk) {
            for c in s.chars() {
                write_escape(output, c)?;
            }
            i += len;
        } else {
            output.write_all(&[b])?;
            i += 1;
        }
    }
    Ok(())
}

/// Length in bytes of the UTF-8 sequence starting with `b`.
const fn utf8_len(b: u8) -> usize {
    if b >= 0xF0 {
        4
    } else if b >= 0xE0 {
        3
    } else if b >= 0xC0 {
        2
    } else {
        1
    }
}

/// Write one non-ASCII character as jq's `\uXXXX` escape(s).
fn write_escape(output: &mut impl Write, c: char) -> io::Result<()> {
    let cp = c as u32;
    if cp <= 0xFFFF {
        write!(output, "\\u{cp:04x}")
    } else {
        // UTF-16 surrogate pair, as jq emits for astral-plane characters.
        let v = cp - 0x1_0000;
        let hi = 0xD800 + (v >> 10);
        let lo = 0xDC00 + (v & 0x3FF);
        write!(output, "\\u{hi:04x}\\u{lo:04x}")
    }
}

/// Copy `input` to `output`, dropping the RS bytes that frame json-seq input.
///
/// The engine's reader would reject the separators outright. Stripping them
/// is lossless for well-formed json-seq: RS never appears inside a JSON
/// text, only between them.
pub fn strip_rs(mut input: impl Read, output: &mut impl Write) -> io::Result<()> {
    let mut buf = [0u8; 8192];
    loop {
        let n = input.read(&mut buf)?;
        if n == 0 {
            break;
        }
        let mut start = 0usize;
        for i in 0..n {
            if buf[i] == RS {
                output.write_all(&buf[start..i])?;
                start = i + 1;
            }
        }
        output.write_all(&buf[start..n])?;
        output.flush()?;
    }
    Ok(())
}

/// A reader dropping the RS bytes that frame json-seq input, as [`strip_rs`]
/// copies them.
pub struct StripRs<R>(pub R);

impl<R: Read> Read for StripRs<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            let n = self.0.read(buf)?;
            if n == 0 {
                return Ok(0);
            }
            let mut kept = 0;
            for i in 0..n {
                if buf[i] != RS {
                    buf[kept] = buf[i];
                    kept += 1;
                }
            }
            // A read of nothing but separators is not the end of the input.
            if kept > 0 {
                return Ok(kept);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Framer, Post, Terminator, strip_rs};
    use std::io::Write as _;

    fn run(input: &str, post: Post) -> String {
        let mut out: Vec<u8> = Vec::new();
        let mut framer = Framer::new(&mut out, post);
        framer.write_all(input.as_bytes()).expect("write succeeds");
        framer.finish().expect("finish succeeds");
        drop(framer);
        String::from_utf8(out).expect("output is UTF-8")
    }

    const PLAIN: Post = Post {
        ascii: false,
        seq: false,
        terminator: Terminator::Newline,
    };
    const ASCII: Post = Post {
        ascii: true,
        seq: false,
        terminator: Terminator::Newline,
    };
    const SEQ: Post = Post {
        ascii: false,
        seq: true,
        terminator: Terminator::Newline,
    };

    #[test]
    fn identity_is_recognised() {
        assert!(PLAIN.is_identity());
        assert!(!ASCII.is_identity());
        assert!(!SEQ.is_identity());
        assert!(
            !Post {
                terminator: Terminator::Nul,
                ..PLAIN
            }
            .is_identity()
        );
    }

    #[test]
    fn plain_passthrough_preserves_bytes() {
        assert_eq!(run("{\"a\":1}\n2\n", PLAIN), "{\"a\":1}\n2\n");
    }

    #[test]
    fn ascii_escapes_non_ascii_as_lowercase_hex() {
        // Matches jq 1.8.1: `jq -ac '"café"'` -> "caf\u00e9"
        assert_eq!(run("\"café\"\n", ASCII), "\"caf\\u00e9\"\n");
    }

    #[test]
    fn ascii_uses_surrogate_pairs_for_astral_characters() {
        // Matches jq 1.8.1: `jq -ac '"😀"'` -> "\ud83d\ude00"
        assert_eq!(run("\"😀\"\n", ASCII), "\"\\ud83d\\ude00\"\n");
    }

    #[test]
    fn ascii_leaves_existing_escapes_and_structure_alone() {
        assert_eq!(
            run("{\"k\":\"\\t\",\"n\":1}\n", ASCII),
            "{\"k\":\"\\t\",\"n\":1}\n"
        );
    }

    #[test]
    fn ascii_reaches_inside_nested_containers() {
        assert_eq!(run("{\"k\":[\"ü\"]}\n", ASCII), "{\"k\":[\"\\u00fc\"]}\n");
    }

    #[test]
    fn seq_prefixes_every_value_with_rs() {
        assert_eq!(run("1\n2\n", SEQ), "\u{1e}1\n\u{1e}2\n");
    }

    #[test]
    fn seq_frames_pretty_printed_values_as_one_unit() {
        // The newlines inside the object are at depth 1 and must not be treated
        // as value boundaries.
        let pretty = "{\n  \"a\": 1\n}\n{\n  \"b\": 2\n}\n";
        let out = run(pretty, SEQ);
        assert_eq!(out.matches('\u{1e}').count(), 2);
        assert!(out.starts_with("\u{1e}{\n  \"a\": 1\n}\n"));
    }

    #[test]
    fn a_newline_inside_a_string_is_not_a_boundary() {
        // Escaped, so it is bytes `\` `n` and never a literal newline...
        assert_eq!(run("\"a\\nb\"\n", SEQ), "\u{1e}\"a\\nb\"\n");
        // ...and an escaped quote must not end the string early.
        assert_eq!(run("\"a\\\"b\"\n", SEQ).matches('\u{1e}').count(), 1);
    }

    #[test]
    fn terminators_follow_the_requested_output_mode() {
        let joined = Post {
            terminator: Terminator::None,
            ..PLAIN
        };
        assert_eq!(run("1\n2\n", joined), "12");
        let nul = Post {
            terminator: Terminator::Nul,
            ..PLAIN
        };
        assert_eq!(run("1\n2\n", nul), "1\u{0}2\u{0}");
    }

    #[test]
    fn a_final_value_without_a_newline_is_still_written() {
        assert_eq!(run("1", PLAIN), "1\n");
    }

    #[test]
    fn strip_rs_removes_separators_only() {
        let mut out: Vec<u8> = Vec::new();
        strip_rs(&b"\x1e1\n\x1e2\n"[..], &mut out).expect("strip succeeds");
        assert_eq!(out, b"1\n2\n");
    }

    #[test]
    fn strip_rs_leaves_unframed_input_untouched() {
        let mut out: Vec<u8> = Vec::new();
        strip_rs(&b"{\"a\":1}\n"[..], &mut out).expect("strip succeeds");
        assert_eq!(out, b"{\"a\":1}\n");
    }
}
