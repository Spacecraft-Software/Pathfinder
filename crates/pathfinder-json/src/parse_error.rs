//! Pathfinder: jq 1.8.1's parse error messages.
//!
//! jaq's parser stays the one that builds values. When it fails, the text of
//! the value it failed on is scanned again by this transcription of jq's own
//! parser (`src/jv_parse.c`, non-streaming), which only tracks the state jq's
//! messages depend on. At the start of a top-level value jq's parser is in its
//! initial state, so scanning from there reproduces jq's message, line and
//! column exactly, provided jq and jaq agree up to that value.
//!
//! jq counts lines from 1 and columns in bytes: a byte advances the column
//! before it is examined, and a newline moves to column 0 of the next line.
//! A string is checked only when its closing quote is reached, so an invalid
//! escape is reported at the closing quote, not at the escape.
use alloc::{string::String, vec::Vec};

/// jq's `MAX_PARSING_DEPTH`: the most arrays and objects open at once.
pub(crate) const MAX_DEPTH: usize = 10000;

/// What jq's parse stack holds.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Frame {
    /// An array, and whether it has an element.
    Arr(bool),
    /// An object, and whether it has a key-value pair.
    Obj(bool),
    /// An object key, after its `:`.
    Key,
}

/// jq's `p->next`: a complete value waiting for a separator.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Next {
    None,
    Str,
    Other,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum St {
    Normal,
    Str,
    Esc,
}

/// The state of jq's parser that its error messages depend on.
pub(crate) struct Scan {
    stack: Vec<Frame>,
    next: Next,
    st: St,
    token: Vec<u8>,
    line: usize,
    column: usize,
}

type Msg = &'static str;

impl Scan {
    /// Start in jq's initial state at `line`, after `column` bytes of it.
    pub(crate) fn new(line: usize, column: usize) -> Self {
        Self {
            stack: Vec::new(),
            next: Next::None,
            st: St::Normal,
            token: Vec::new(),
            line,
            column,
        }
    }

    /// Start at `start` in `text`, counting jq's line and column up to it.
    pub(crate) fn at(text: &[u8], start: usize) -> Self {
        let before = &text[..start];
        let line = 1 + before.iter().filter(|&&c| c == b'\n').count();
        let column = before.iter().rev().take_while(|&&c| c != b'\n').count();
        Self::new(line, column)
    }

    /// jq's current line.
    pub(crate) fn line(&self) -> usize {
        self.line
    }

    /// jq's current column.
    pub(crate) fn column(&self) -> usize {
        self.column
    }

    /// Scan one byte; jq's error message for it, if any.
    pub(crate) fn byte(&mut self, ch: u8) -> Option<String> {
        self.scan(ch)
            .err()
            .map(|m| alloc::format!("{m} at line {}, column {}", self.line, self.column))
    }

    /// Scan bytes until jq's first error.
    pub(crate) fn bytes(&mut self, text: &[u8]) -> Option<String> {
        text.iter().find_map(|&c| self.byte(c))
    }

    /// The end of the input; jq's error message for it, if any.
    pub(crate) fn eof(&mut self) -> Option<String> {
        let (line, column) = (self.line, self.column);
        let at = |m: &str| alloc::format!("{m} at EOF at line {line}, column {column}");
        if self.st != St::Normal {
            Some(at("Unfinished string"))
        } else if let Err(m) = self.check_literal() {
            Some(at(m))
        } else if !self.stack.is_empty() {
            Some(at("Unfinished JSON term"))
        } else {
            None
        }
    }

    /// jq's `scan`, without building values.
    fn scan(&mut self, ch: u8) -> Result<(), Msg> {
        self.column += 1;
        if ch == b'\n' {
            self.line += 1;
            self.column = 0;
        }
        match self.st {
            St::Normal => {
                let literal =
                    !matches!(ch, b' ' | b'\t' | b'\r' | b'\n' | b'"') && !is_structure(ch);
                if literal {
                    self.token.push(ch);
                    return Ok(());
                }
                self.check_literal()?;
                self.check_done();
                if ch == b'"' {
                    self.st = St::Str;
                } else if is_structure(ch) {
                    self.token_char(ch)?;
                }
                self.check_done();
            }
            St::Str if ch == b'"' => {
                self.found_string()?;
                self.st = St::Normal;
                self.check_done();
            }
            st => {
                self.token.push(ch);
                self.st = if ch == b'\\' && st == St::Str {
                    St::Esc
                } else {
                    St::Str
                };
            }
        }
        Ok(())
    }

    /// jq's `parse_check_done`: a complete top-level value is emitted.
    fn check_done(&mut self) {
        if self.stack.is_empty() {
            self.next = Next::None;
        }
    }

    /// jq's `value`.
    fn value(&mut self, v: Next) -> Result<(), Msg> {
        if self.next != Next::None {
            return Err("Expected separator between values");
        }
        self.next = v;
        Ok(())
    }

    fn top(&self) -> Option<Frame> {
        self.stack.last().copied()
    }

    /// jq's `parse_token`.
    fn token_char(&mut self, ch: u8) -> Result<(), Msg> {
        match ch {
            b'[' | b'{' => {
                if self.stack.len() >= MAX_DEPTH {
                    return Err("Exceeds depth limit for parsing");
                }
                if self.next != Next::None {
                    return Err("Expected separator between values");
                }
                self.stack.push(if ch == b'[' {
                    Frame::Arr(false)
                } else {
                    Frame::Obj(false)
                });
            }
            b':' => {
                if self.next == Next::None {
                    return Err("Expected string key before ':'");
                }
                if !matches!(self.top(), Some(Frame::Obj(_))) {
                    return Err("':' not as part of an object");
                }
                if self.next != Next::Str {
                    return Err("Object keys must be strings");
                }
                self.stack.push(Frame::Key);
                self.next = Next::None;
            }
            b',' => {
                if self.next == Next::None {
                    return Err("Expected value before ','");
                }
                match self.top() {
                    None => return Err("',' not as part of an object or array"),
                    Some(Frame::Arr(_)) => *self.stack.last_mut().unwrap() = Frame::Arr(true),
                    Some(Frame::Key) => self.set_key(),
                    Some(Frame::Obj(_)) => return Err("Objects must consist of key:value pairs"),
                }
                self.next = Next::None;
            }
            b']' => {
                match self.top() {
                    Some(Frame::Arr(full)) => {
                        if self.next == Next::None && full {
                            return Err("Expected another array element");
                        }
                    }
                    _ => return Err("Unmatched ']'"),
                }
                self.stack.pop();
                self.next = Next::Other;
            }
            b'}' => {
                let Some(top) = self.top() else {
                    return Err("Unmatched '}'");
                };
                if self.next != Next::None {
                    if top != Frame::Key {
                        return Err("Objects must consist of key:value pairs");
                    }
                    self.set_key();
                } else {
                    match top {
                        Frame::Obj(false) => (),
                        Frame::Obj(true) => return Err("Expected another key-value pair"),
                        _ => return Err("Unmatched '}'"),
                    }
                }
                self.stack.pop();
                self.next = Next::Other;
            }
            _ => (),
        }
        Ok(())
    }

    /// Store a key-value pair: pop the key, the object has a pair.
    fn set_key(&mut self) {
        self.stack.pop();
        if let Some(obj) = self.stack.last_mut() {
            *obj = Frame::Obj(true);
        }
    }

    /// jq's `check_literal`.
    fn check_literal(&mut self) -> Result<(), Msg> {
        let t = &self.token;
        if t.is_empty() {
            return Ok(());
        }
        let pattern: Option<&[u8]> = match t[0] {
            b't' => Some(b"true"),
            b'f' => Some(b"false"),
            b'\'' => return Err("Invalid string literal; expected \", but got '"),
            b'n' if t.get(1) == Some(&b'u') => Some(b"null"),
            _ => None,
        };
        match pattern {
            Some(p) if t[..] != *p => return Err("Invalid literal"),
            Some(_) => (),
            // jq hands the token to decNumber as a C string.
            None if !is_number(t.split(|&c| c == 0).next().unwrap_or_default()) => {
                return Err("Invalid numeric literal")
            }
            None => (),
        }
        self.value(Next::Other)?;
        self.token.clear();
        Ok(())
    }

    /// jq's `found_string`: check the escapes and control characters.
    fn found_string(&mut self) -> Result<(), Msg> {
        let s = &self.token;
        let hex4 = |i: usize| -> Option<u32> {
            let h = s.get(i..i + 4)?;
            let h = core::str::from_utf8(h).ok()?;
            h.bytes()
                .all(|c| c.is_ascii_hexdigit())
                .then(|| u32::from_str_radix(h, 16).ok())?
        };
        let mut i = 0;
        while i < s.len() {
            let c = s[i];
            i += 1;
            if c == b'\\' {
                let Some(&e) = s.get(i) else {
                    return Err("Expected escape character at end of string");
                };
                i += 1;
                match e {
                    b'\\' | b'"' | b'/' | b'b' | b'f' | b't' | b'n' | b'r' => (),
                    b'u' => {
                        if i + 4 > s.len() {
                            return Err("Invalid \\uXXXX escape");
                        }
                        let cp = hex4(i).ok_or("Invalid characters in \\uXXXX escape")?;
                        i += 4;
                        if (0xD800..=0xDBFF).contains(&cp) {
                            let pair = Err("Invalid \\uXXXX\\uXXXX surrogate pair escape");
                            if i + 6 > s.len() || s[i] != b'\\' || s[i + 1] != b'u' {
                                return pair;
                            }
                            if !hex4(i + 2).is_some_and(|lo| (0xDC00..=0xDFFF).contains(&lo)) {
                                return pair;
                            }
                            i += 6;
                        }
                    }
                    _ => return Err("Invalid escape"),
                }
            } else if c < 0x20 {
                return Err(
                    "Invalid string: control characters from U+0000 through U+001F must be escaped",
                );
            }
        }
        self.value(Next::Str)?;
        self.token.clear();
        Ok(())
    }
}

fn is_structure(c: u8) -> bool {
    matches!(c, b'[' | b',' | b']' | b'{' | b':' | b'}')
}

/// Whether decNumber's `decNumberFromString` reads `t` without a syntax
/// error, and jq keeps it (jq rejects a NaN with a payload).
fn is_number(t: &[u8]) -> bool {
    let unsigned = t.strip_prefix(b"-").or_else(|| t.strip_prefix(b"+"));
    let t = unsigned.unwrap_or(t);
    let mantissa = t
        .iter()
        .scan(false, |dot, &c| match c {
            b'0'..=b'9' => Some(()),
            b'.' if !*dot => {
                *dot = true;
                Some(())
            }
            _ => None,
        })
        .count();
    let (m, rest) = t.split_at(mantissa);
    if !m.iter().any(u8::is_ascii_digit) {
        // No digits: an infinity or a NaN, unless there was a point.
        if rest.is_empty() || m.contains(&b'.') {
            return false;
        }
        let lower = rest.to_ascii_lowercase();
        if lower == b"inf" || lower == b"infinity" {
            return true;
        }
        let nan = lower.strip_prefix(b"s").unwrap_or(&lower);
        // A payload of zeros is no payload.
        return nan
            .strip_prefix(b"nan")
            .is_some_and(|p| p.iter().all(|&c| c == b'0'));
    }
    match rest {
        [] => true,
        [b'e' | b'E', exp @ ..] => {
            let exp = exp
                .strip_prefix(b"-")
                .or_else(|| exp.strip_prefix(b"+"))
                .unwrap_or(exp);
            !exp.is_empty() && exp.iter().all(u8::is_ascii_digit)
        }
        _ => false,
    }
}

#[test]
fn numbers() {
    for t in [
        "1", "-1", "+1", "1.", ".5", "-.5", "1e5", "1.e5", "0.e1", "1E+5", "nan", "-NaN",
    ] {
        assert!(is_number(t.as_bytes()), "{t}");
    }
    for t in ["sNaN", "NaN0", "NaN00", "inf", "+Infinity", "00"] {
        assert!(is_number(t.as_bytes()), "{t}");
    }
    for t in [
        "", ".", "-", "+", "-.", "e5", ".e1", "1e", "1e+", "1.2.3", "NaN1", "NaN01",
    ] {
        assert!(!is_number(t.as_bytes()), "{t}");
    }
    for t in [
        "infinit", "0x10", "1_0", "--1", "+-1", "True", "nan0.", "1ee5", "1e5.5",
    ] {
        assert!(!is_number(t.as_bytes()), "{t}");
    }
}

#[test]
fn messages() {
    let err = |s: &str| {
        let mut scan = Scan::new(1, 0);
        scan.bytes(s.as_bytes()).or_else(|| scan.eof())
    };
    // Each expected message was produced by jq 1.8.1.
    let cases = [
        ("{", "Unfinished JSON term at EOF at line 1, column 1"),
        ("1 2 ]", "Unmatched ']' at line 1, column 5"),
        ("[1,]", "Expected another array element at line 1, column 4"),
        (
            "{\"a\" 1}",
            "Expected separator between values at line 1, column 7",
        ),
        ("{1:2}", "Object keys must be strings at line 1, column 3"),
        ("{\"a\":}", "Unmatched '}' at line 1, column 6"),
        ("truex", "Invalid literal at EOF at line 1, column 5"),
        ("1 2 3x 4", "Invalid numeric literal at line 1, column 7"),
        ("\"\\u12\"", "Invalid \\uXXXX escape at line 1, column 6"),
        (
            "{'a': 123}",
            "Invalid string literal; expected \", but got ' at line 1, column 5",
        ),
        (
            "{\"a\",1}",
            "Objects must consist of key:value pairs at line 1, column 5",
        ),
        (
            "1\n2\n{\n",
            "Unfinished JSON term at EOF at line 4, column 0",
        ),
        ("1}", "Unmatched '}' at line 1, column 2"),
        ("[1:2]", "':' not as part of an object at line 1, column 3"),
    ];
    for (input, msg) in cases {
        assert_eq!(err(input).as_deref(), Some(msg), "{input}");
    }
    assert_eq!(err("[1.] .5 nan 01 \"\\ud83d\\ude00\""), None);
}
