//! JSON support.
use crate::{Map, Num, Val};
use alloc::{string::ToString, vec::Vec};
use core::fmt::{self, Formatter};
use hifijson::token::{Expect, Lex};
use hifijson::{LexAlloc, Read as _, SliceLexer};
// Pathfinder: jq's parse error messages.
use crate::parse_error;
use alloc::string::String;
#[cfg(feature = "std")]
use std::io;

/// Eat whitespace, then peek at next character.
///
/// Pathfinder: jq reads no comments; to jq 1.8.1, `#` begins a literal.
fn ws_tk<L: Lex>(lexer: &mut L) -> Option<u8> {
    let mut hash = false;
    loop {
        lexer.eat_whitespace();
        match lexer.peek_next() {
            Some(b'#') if !hash => {
                hash = true;
                lexer.skip_until(|_| true)
            }
            next => return next,
        }
    }
}

/// Pathfinder: jq reads `nan` and `infinity`/`inf` in any case, signed or
/// not; jaq reads only `NaN` and `Infinity`. The word is read whole, because
/// a streaming lexer cannot step back after a failed prefix (`null`/`nan`).
fn word<L: Lex>(lexer: &mut L) -> Vec<u8> {
    let mut w = Vec::new();
    while let Some(c) = lexer.peek_next().filter(u8::is_ascii_alphabetic) {
        if w.len() == 9 {
            break;
        }
        lexer.take_next();
        w.push(c);
    }
    w
}

fn special(w: &[u8]) -> Option<f64> {
    match w.to_ascii_lowercase().as_slice() {
        // Pathfinder: decNumber also reads a signalling NaN.
        b"nan" | b"snan" => Some(f64::NAN),
        b"inf" | b"infinity" => Some(f64::INFINITY),
        _ => None,
    }
}

/// Pathfinder: [`special`] for the word at the lexer, which for a NaN takes
/// a payload of zeros with it, as decNumber does (`NaN0` is NaN to jq).
fn special_word<L: Lex>(w: &[u8], lexer: &mut L) -> Option<f64> {
    let f = special(w)?;
    if f.is_nan() {
        while lexer.peek_next() == Some(b'0') {
            lexer.take_next();
        }
    }
    Some(f)
}

/// Pathfinder: the UTF-8 byte order mark jq skips at the start of its input.
const BOM: &[u8] = b"\xEF\xBB\xBF";

/// Parse error.
///
/// Pathfinder: where it is known, the error carries jq 1.8.1's message for
/// the same input, which is then all it displays.
#[derive(Debug)]
pub struct Error(usize, hifijson::Error, Option<String>);

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match &self.2 {
            Some(jq) => f.write_str(jq),
            None => write!(f, "byte offset {}: {}", self.0, self.1),
        }
    }
}

impl Error {
    /// Pathfinder: jq's message for the value starting at `start` in `text`,
    /// where jaq failed at `at`.
    fn jq(text: &[u8], start: usize, at: usize, e: hifijson::Error) -> Self {
        let mut scan = parse_error::Scan::at(text, start);
        let jq = scan.bytes(&text[start..]).or_else(|| scan.eof());
        // jq reads the value without error: report jaq's error jq's way.
        let jq = jq.unwrap_or_else(|| {
            let scan = parse_error::Scan::at(text, at);
            alloc::format!("{e} at line {}, column {}", scan.line(), scan.column() + 1)
        });
        Self(at, e, Some(jq))
    }

    /// Pathfinder: the message without the position, as jq's `fromjson`
    /// reports some errors.
    fn message(msg: &str) -> Self {
        Self(
            0,
            hifijson::Error::Token(Expect::Value),
            Some(msg.to_string()),
        )
    }
}

#[cfg(feature = "std")]
impl std::error::Error for Error {}

/// Parse exactly one JSON value.
pub fn parse_single(slice: &[u8]) -> Result<Val, Error> {
    let offset = |rest: &[u8]| rest.as_ptr() as usize - slice.as_ptr() as usize;
    let mut lexer = SliceLexer::new(slice);
    lexer
        .exactly_one(ws_tk, |next, lexer| parse(next, lexer, 0))
        .map_err(|e| Error(offset(lexer.as_slice()), e, None))
}

/// Parse a sequence of JSON values.
pub fn parse_many(slice: &[u8]) -> impl Iterator<Item = Result<Val, Error>> + '_ {
    // Pathfinder: values as jq reads them; see `parse_many_at`.
    values_jq(slice, |_| ())
}

/// Pathfinder: [`parse_many`], recording in `at` the offset in `slice` the
/// lexer has read up to, so that an error can name the line as jq does.
///
/// Values are read as jq reads them, and an error carries jq's message.
/// After an error there are no more values: jq stops reading the input.
pub fn parse_many_at<'a>(
    slice: &'a [u8],
    at: &'a core::cell::Cell<Option<usize>>,
) -> impl Iterator<Item = Result<Val, Error>> + 'a {
    values_jq(slice, |o| at.set(Some(o)))
}

/// Pathfinder: values as jq reads them, telling `at` the offset read up to.
fn values_jq<'a>(
    slice: &'a [u8],
    mut at: impl FnMut(usize) + 'a,
) -> impl Iterator<Item = Result<Val, Error>> + 'a {
    let skip = if slice.starts_with(BOM) { BOM.len() } else { 0 };
    let body = &slice[skip..];
    let offset = move |rest: &[u8]| rest.as_ptr() as usize - body.as_ptr() as usize;
    let mut lexer = SliceLexer::new(body);
    let mut failed = false;
    core::iter::from_fn(move || {
        if failed {
            return None;
        }
        let v = ws_tk(&mut lexer).map(|next| {
            let start = offset(lexer.as_slice());
            parse_top(next, &mut lexer).map_err(|e| {
                failed = true;
                Error::jq(body, start, offset(lexer.as_slice()), e)
            })
        });
        at(skip + offset(lexer.as_slice()));
        v
    })
}

/// Pathfinder: exactly one JSON value, as jq 1.8.1's `fromjson` reads it,
/// failing with jq's message for it.
pub fn parse_jq_single(slice: &[u8]) -> Result<Val, Error> {
    let mut values = parse_many(slice);
    let v = match values.next() {
        None => Err(Error::message("Expected JSON value")),
        Some(Ok(v)) => match values.next() {
            None => Ok(v),
            Some(Ok(_)) => Err(Error::message("Unexpected extra JSON values")),
            Some(Err(e)) => Err(e),
        },
        Some(Err(e)) => Err(e),
    };
    // jq prints the text with `%s`, which stops at a NUL.
    let text = slice.split(|&c| c == 0).next().unwrap_or_default();
    v.map_err(|Error(at, e, jq)| {
        let jq = jq.unwrap_or_default();
        let text = alloc::string::String::from_utf8_lossy(text);
        Error(at, e, Some(alloc::format!("{jq} (while parsing '{text}')")))
    })
}

/// Pathfinder: how a stream reader lets jq's message for a parse error see
/// the text of the value that failed. Implemented by jaq's line reader.
pub trait Recall {
    /// The value about to be parsed starts with the last byte read.
    fn mark(&self);
    /// The value has been parsed.
    fn unmark(&self);
    /// Ask for the bytes read since the mark at the next refill.
    fn flush(&self);
    /// How often the reader has handed out a new chunk, or found the end.
    fn refills(&self) -> usize;
    /// The bytes read from the mark to the end of the last chunk handed out,
    /// and jq's line of the mark and its column before it.
    fn marked(&self) -> (Vec<u8>, usize, usize);
}

#[cfg(feature = "std")]
/// Pathfinder: [`read_many`] as jq reads values, with jq's message for an
/// error, after which there are no more values.
pub fn read_many_jq<'a>(
    read: impl io::BufRead + 'a,
    recall: &'a dyn Recall,
) -> impl Iterator<Item = io::Result<Val>> + 'a {
    // Nothing is read before the first value is asked for, as in `read_many`.
    let (mut read, mut lexer, mut bom) = (Some(read), None, 0);
    let mut failed = false;
    core::iter::from_fn(move || {
        if failed {
            return None;
        }
        let lexer = lexer.get_or_insert_with(|| {
            let mut read = read.take().expect("read is taken once");
            if read.fill_buf().is_ok_and(|b| b.starts_with(BOM)) {
                read.consume(BOM.len());
                bom = BOM.len();
            }
            hifijson::IterLexer::new(read.bytes())
        });
        let v = ws_tk(lexer).map(|next| {
            recall.mark();
            parse_top(next, lexer)
        });
        if let Some(e) = lexer.error.take() {
            failed = true;
            return Some(Err(e));
        }
        Some(match v? {
            Ok(v) => {
                recall.unmark();
                Ok(v)
            }
            Err(e) => {
                failed = true;
                let msg = jq_stream_error(lexer, recall, bom, e);
                lexer
                    .error
                    .take()
                    .map_or_else(|| Err(io::Error::new(io::ErrorKind::InvalidData, msg)), Err)
            }
        })
    })
}

#[cfg(feature = "std")]
/// Pathfinder: jq's message for an error in the marked value of a stream.
///
/// The bytes read since the mark that are still in the reader's chunk reach
/// [`Recall::marked`] when the reader hands out its next chunk, once asked
/// for, so those up to there are read and dropped; jq's parser then reads on
/// until its error.
fn jq_stream_error<E, I: Iterator<Item = Result<u8, E>>>(
    lexer: &mut hifijson::IterLexer<E, I>,
    recall: &dyn Recall,
    bom: usize,
    e: hifijson::Error,
) -> String {
    recall.flush();
    let refills = recall.refills();
    let first = loop {
        let c = lexer.take_next();
        if recall.refills() != refills || c.is_none() {
            break c;
        }
    };
    let (mut text, line, column) = recall.marked();
    text.extend(first);
    // jq skips the byte order mark before it counts columns.
    let column = if line == 1 { column - bom } else { column };
    let mut scan = parse_error::Scan::new(line, column);
    let jq = scan.bytes(&text).or_else(|| {
        let mut rest = core::iter::from_fn(|| lexer.take_next());
        rest.find_map(|c| scan.byte(c))
    });
    jq.or_else(|| scan.eof())
        .unwrap_or_else(|| alloc::format!("{e} at line {}, column {}", scan.line(), scan.column()))
}

#[cfg(feature = "std")]
/// Read a sequence of JSON values.
pub fn read_many<'a>(read: impl io::BufRead + 'a) -> impl Iterator<Item = io::Result<Val>> + 'a {
    let invalid_data = |e| io::Error::new(io::ErrorKind::InvalidData, e);
    // Pathfinder: nothing is read before the first value is asked for, as in
    // jq, which has read no input when `jq -n 'error'` reports `<unknown>`.
    let (mut read, mut lexer) = (Some(read), None);
    core::iter::from_fn(move || {
        let lexer = lexer.get_or_insert_with(|| {
            let mut read = read.take().expect("read is taken once");
            if read.fill_buf().is_ok_and(|b| b.starts_with(BOM)) {
                read.consume(BOM.len());
            }
            hifijson::IterLexer::new(read.bytes())
        });
        let v = ws_tk(lexer).map(|next| parse_top(next, lexer).map_err(invalid_data));
        // always return I/O error if present, regardless of the output value!
        lexer.error.take().map(Err).or(v)
    })
}

/// Parse a single number.
pub fn parse_single_num(slice: &[u8]) -> Option<Num> {
    Some(match slice {
        b"Infinity" => Num::Float(f64::INFINITY),
        b"NaN" => Num::Float(f64::NAN),
        _ => {
            let mut lexer = SliceLexer::new(slice);
            parse_num(&mut lexer)
                .ok()
                .filter(|_| lexer.as_slice().is_empty())?
        }
    })
}

/// Parse a JSON string as byte or text string, preserving invalid UTF-8 as-is.
///
/// Pathfinder: kept out of line, as it was before `parse` grew a depth.
#[inline(never)]
fn parse_string<L: LexAlloc>(lexer: &mut L, bytes: bool) -> Result<Vec<u8>, hifijson::Error> {
    let on_string = |bytes: &mut L::Bytes, out: &mut Vec<u8>| {
        out.extend(bytes.as_ref());
        Ok(())
    };
    let s = lexer.str_fold(Vec::new(), on_string, |lexer, out| {
        use hifijson::escape::Error;
        match lexer.take_next().ok_or(Error::Eof)? {
            b'u' if bytes => Err(Error::InvalidKind(b'u'))?,
            b'x' if bytes => out.push(lexer.hex()?),
            c => out.extend(lexer.escape(c)?.encode_utf8(&mut [0; 4]).as_bytes()),
        }
        Ok(())
    });
    s.map_err(hifijson::Error::Str)
}

fn parse_num<L: LexAlloc>(lexer: &mut L) -> Result<Num, hifijson::Error> {
    // Pathfinder: a number as decNumber reads jq's literal, which JSON's
    // grammar does not cover: `1.`, `.5`, `-.5`, `1.e3` (M23). A sign, then
    // digits and a point with a digit on either side or both, then an
    // optional exponent.
    let mut num = String::new();
    let mut is_int = true;
    if let Some(c @ (b'+' | b'-')) = lexer.peek_next() {
        lexer.take_next();
        num.push(char::from(c));
    }
    let digits = |lexer: &mut L, num: &mut String| {
        let mut n = 0;
        while let Some(c) = lexer.peek_next().filter(u8::is_ascii_digit) {
            lexer.take_next();
            num.push(char::from(c));
            n += 1;
        }
        n
    };
    let mut seen = digits(lexer, &mut num);
    if lexer.peek_next() == Some(b'.') {
        lexer.take_next();
        num.push('.');
        is_int = false;
        seen += digits(lexer, &mut num);
    }
    if seen > 0 && matches!(lexer.peek_next(), Some(b'e' | b'E')) {
        lexer.take_next();
        num.push('e');
        is_int = false;
        if let Some(c @ (b'+' | b'-')) = lexer.peek_next() {
            lexer.take_next();
            num.push(char::from(c));
        }
        if digits(lexer, &mut num) == 0 {
            Err(hifijson::num::Error::ExpectedDigit)?;
        }
    }
    if seen == 0 && num.contains('.') {
        Err(hifijson::num::Error::ExpectedDigit)?;
    }
    let num = num.as_str();
    Ok(match num {
        "+" => Num::Float(
            special_word(&word(lexer), lexer).ok_or(hifijson::num::Error::ExpectedDigit)?,
        ),
        "-" => Num::Float(
            -special_word(&word(lexer), lexer).ok_or(hifijson::num::Error::ExpectedDigit)?,
        ),
        _ if seen > 0 => {
            // A leading `+` is not kept: decNumber's text has none, and
            // negation writes a sign before the text (`-+.5`).
            let num = num.strip_prefix('+').unwrap_or(num);
            // Pathfinder: jq keeps `-0` as written; an integer would lose the sign.
            if num == "-0" {
                Num::Dec(num.to_string().into())
            } else if is_int {
                Num::from_str_radix(num, 10).unwrap()
            } else {
                Num::Dec(num.to_string().into())
            }
        }
        _ => Err(hifijson::num::Error::ExpectedDigit)?,
    })
}

/// Pathfinder: a top-level value as jq reads it.
///
/// jq reads a number or literal up to the next whitespace, quote or
/// structural character, all of which is the token: `truex` and `1.5.` are
/// errors, not `true` and `1.5` followed by an error. And a top-level value
/// ended by `]`, `}`, `,` or `:` is lost to the error that byte causes.
fn parse_top<L: LexAlloc>(next: u8, lexer: &mut L) -> Result<Val, hifijson::Error> {
    let v = parse(next, lexer, 0)?;
    if !matches!(next, b'"' | b'[' | b'{') {
        match lexer.peek_next() {
            None | Some(b' ' | b'\t' | b'\r' | b'\n' | b'"' | b'[' | b'{') => (),
            Some(_) => Err(Expect::Value)?,
        }
    }
    Ok(v)
}

/// Parse a JSON value, given an initial non-whitespace character and a lexer.
///
/// If the underlying lexer reads input fallibly (for example [`hifijson::IterLexer`]),
/// the error returned by this function might be misleading.
/// In that case, always check whether the lexer contains an error.
fn parse<L: LexAlloc>(next: u8, lexer: &mut L, depth: usize) -> Result<Val, hifijson::Error> {
    // Pathfinder: jq's depth limit, which also keeps deep input from
    // overflowing the stack.
    if matches!(next, b'[' | b'{') && depth >= parse_error::MAX_DEPTH {
        return Err(hifijson::Error::Depth);
    }
    Ok(match next {
        b't' if lexer.strip_prefix(b"true") => Val::Bool(true),
        b'f' if lexer.strip_prefix(b"false") => Val::Bool(false),
        // Pathfinder: jq reads no byte strings (`b"…"`).
        b'n' | b'N' | b'i' | b'I' | b's' | b'S' => match word(lexer) {
            w if w == b"null" => Val::Null,
            w => Val::Num(Num::Float(special_word(&w, lexer).ok_or(Expect::Value)?)),
        },
        b'0'..=b'9' | b'+' | b'-' | b'.' => Val::Num(parse_num(lexer)?),
        b'"' => Val::utf8_str(parse_string(lexer.discarded(), false)?),
        b'[' => Val::Arr({
            let mut arr = Vec::new();
            lexer.discarded().seq(b']', ws_tk, |next, lexer| {
                arr.push(parse(next, lexer, depth + 1)?);
                Ok::<_, hifijson::Error>(())
            })?;
            arr.into()
        }),
        b'{' => Val::obj({
            let mut obj = Map::default();
            lexer.discarded().seq(b'}', ws_tk, |next, lexer| {
                // Pathfinder: jq reads only strings as object keys.
                if next != b'"' {
                    Err(Expect::String)?;
                }
                let key = parse(next, lexer, depth + 1)?;
                lexer.expect(ws_tk, b':').ok_or(Expect::Colon)?;
                let value = parse(ws_tk(lexer).ok_or(Expect::Value)?, lexer, depth + 1)?;
                obj.insert(key, value);
                Ok::<_, hifijson::Error>(())
            })?;
            obj
        }),
        _ => Err(Expect::Value)?,
    })
}
