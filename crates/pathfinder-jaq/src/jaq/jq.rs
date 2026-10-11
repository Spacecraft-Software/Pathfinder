//! Pathfinder: an uncaught error, reported and survived as jq 1.8.1 does.
//!
//! jq writes an error that escapes the filter as
//! `jq: error (at <stdin>:3): Cannot index number with string "a"`, drops the
//! rest of that input's outputs and goes on to the next input. Its exit status
//! is decided by the last input (`process` and `main` in jq's `src/main.c`).
//!
//! The line in the location is the number of complete lines jq's reader has
//! fetched when the error is reported. jq reads with `fgets` into a 4096-byte
//! buffer, so it fetches up to a newline or 4095 bytes at a time, and fetches
//! the chunk holding the last byte its parser needed: the closing character
//! of a string, array or object, the delimiter after a number or literal.
//! (On a stream jaq's lexer needs no delimiter after `true`, `false` or
//! `null`, so one that ends exactly at a 4095-byte boundary of a longer line
//! is reported a line short of jq.)
use core::cell::{Cell, RefCell};
use jaq_all::json::{Error, Val};
use std::io::{self, BufRead, Read};

/// How an input ended, as jq's `process` returns it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Ret {
    /// No output.
    #[default]
    NoOutput,
    /// The last output was true-ish, or `--exit-status` is off.
    Ok,
    /// The last output was `false` or `null`, under `--exit-status`.
    NullKind,
    /// An uncaught error.
    Error,
}

/// What jq's `main` remembers across inputs to decide the exit status.
#[derive(Clone, Copy, Debug, Default)]
pub struct Status {
    /// How the most recent input ended.
    pub ret: Ret,
    /// Whether the most recent input with output ended true-ish.
    pub last: Option<bool>,
}

impl Status {
    /// Record how an input ended.
    pub fn end(&mut self, ret: Ret) {
        self.ret = ret;
        if matches!(ret, Ret::Ok | Ret::NullKind) {
            self.last = Some(ret == Ret::Ok);
        }
    }

    /// jq's exit status.
    pub fn code(&self, exit_status: bool) -> u8 {
        match (self.ret, exit_status) {
            (Ret::Error, _) => 5,
            (_, false) | (Ret::Ok, true) => 0,
            (Ret::NullKind, true) => 1,
            (Ret::NoOutput, true) => match self.last {
                None => 4,
                Some(false) => 1,
                Some(true) => 0,
            },
        }
    }
}

/// The most jq's `fgets` fetches at once.
const CHUNK: usize = 4095;

/// Pathfinder: the variable in which Pathfinder describes the files it has
/// concatenated onto stdin, so that their names and lines are reported as jq
/// reports them. A stock jaq ignores it.
///
/// One record per file, in order: `<bytes>:<name length>:<name><message
/// length>:<message>`, lengths in bytes. A non-empty message means the file
/// could not be read; it holds no bytes, and jq's `jq: error: <message>` is
/// written when the reader reaches it. The last file's byte count is not
/// used: it extends to the end of stdin.
pub const INPUTS_ENV: &str = "PATHFINDER_INPUTS";

/// Pathfinder: one of the files concatenated onto stdin.
#[derive(Debug, PartialEq, Eq)]
pub struct Segment {
    name: String,
    len: u64,
    failure: Option<String>,
}

impl Segment {
    /// The files described by [`INPUTS_ENV`], which is removed so that
    /// neither `$ENV` nor `env` shows it. Malformed, it is ignored.
    pub fn take_env() -> Vec<Self> {
        let Some(v) = std::env::var_os(INPUTS_ENV) else {
            return Vec::new();
        };
        std::env::remove_var(INPUTS_ENV);
        // Pathfinder writes it in UTF-8.
        let v = v.into_string().unwrap_or_default();
        Self::parse(v.as_bytes()).unwrap_or_default()
    }

    fn parse(mut s: &[u8]) -> Option<Vec<Self>> {
        fn number(s: &mut &[u8]) -> Option<u64> {
            let i = s.iter().position(|&b| b == b':')?;
            let n = core::str::from_utf8(&s[..i]).ok()?.parse().ok()?;
            *s = &s[i + 1..];
            Some(n)
        }
        fn text(s: &mut &[u8]) -> Option<String> {
            let n = usize::try_from(number(s)?).ok()?;
            let t = String::from_utf8_lossy(s.get(..n)?).into_owned();
            *s = &s[n..];
            Some(t)
        }
        let mut segments = Vec::new();
        while !s.is_empty() {
            let len = number(&mut s)?;
            let name = text(&mut s)?;
            let failure = Some(text(&mut s)?).filter(|m| !m.is_empty());
            segments.push(Self { name, len, failure });
        }
        Some(segments)
    }
}

/// How far the input has been read, for the location of an error.
#[derive(Default)]
pub struct Position {
    /// Complete lines fetched from a stream; `None` until it is first read.
    lines: Cell<Option<usize>>,
    /// Offset read up to in an in-memory file.
    at: Cell<Option<usize>>,
    /// Offset last counted to, the newlines before it, and where its line
    /// starts (errors arrive in reading order).
    counted: Cell<(usize, usize, usize)>,
    /// [`Lines`]' read position in its buffer, and the offset in the input
    /// of the buffer's start: kept here so that a mark can be placed without
    /// counting every byte.
    pos: Cell<usize>,
    base: Cell<u64>,
    /// Lines before the chunk [`Lines`] is handing out, and the offset where
    /// the line holding its start begins.
    chunk_line: Cell<(usize, u64)>,
    /// The offset where the value being parsed starts, jq's line there and
    /// its column before it (see [`jaq_all::json::read::Recall`]).
    mark: Cell<Option<(u64, usize, usize)>>,
    /// Bytes read from the mark that [`Lines`] has let go of, the offset
    /// they reach, and whether the rest of its chunk is asked for.
    kept: RefCell<Vec<u8>>,
    kept_end: Cell<u64>,
    flush: Cell<bool>,
    /// How often [`Lines`] has handed out a chunk or found the end.
    refills: Cell<usize>,
    /// Pathfinder: the files a stream is made of, if it is several, the one
    /// being read, and how many could not be read so far.
    segments: Vec<Segment>,
    segment: Cell<usize>,
    failures: Cell<usize>,
    /// The lines of the files before the one being read: jq's parser counts
    /// on across files, where its location starts again.
    lines_before: Cell<usize>,
}

/// Pathfinder: what jq's message for a parse error in a stream needs.
impl jaq_all::json::read::Recall for Position {
    fn mark(&self) {
        // The lexer has just read the value's first byte.
        let at = self.base.get() + self.pos.get() as u64 - 1;
        let (before, line_start) = self.chunk_line.get();
        let column = (at - line_start) as usize;
        self.mark.set(Some((at, before + 1, column)));
        self.kept.borrow_mut().clear();
        self.kept_end.set(at);
    }

    fn unmark(&self) {
        self.mark.set(None);
    }

    fn flush(&self) {
        self.flush.set(true);
    }

    fn refills(&self) -> usize {
        self.refills.get()
    }

    fn marked(&self) -> (Vec<u8>, usize, usize) {
        let (_, line, column) = self.mark.get().unwrap_or_default();
        (self.kept.take(), line, column)
    }
}

impl Position {
    /// A stream made of `segments`, as jq reads several files.
    pub fn with_segments(segments: Vec<Segment>) -> Self {
        Self {
            segments,
            ..Self::default()
        }
    }

    /// The offset read up to in an in-memory file, set by its parser.
    pub fn at(&self) -> &Cell<Option<usize>> {
        &self.at
    }

    /// Whether a file of the stream could not be read. jq then stops after
    /// the input being read, and exits with 2.
    pub fn failed(&self) -> bool {
        self.failures.get() > 0
    }

    /// jq's `<name>:<line>`, or `<unknown>` before any input has been read.
    /// In a stream of several files, `name` is the file being read.
    pub fn location(&self, name: &str, bytes: Option<&[u8]>) -> String {
        let name = match self.segments.get(self.segment.get()) {
            Some(s) => &s.name,
            None => name,
        };
        let lines = match (bytes, self.at.get()) {
            (Some(bytes), Some(at)) => Some(self.file_lines(bytes, at)),
            _ => self.lines.get(),
        };
        lines.map_or_else(|| "<unknown>".into(), |n| format!("{name}:{n}"))
    }

    /// Start reading segment `i`, as jq opens the next file: an unreadable
    /// one is reported then. Returns the offset at which it ends, if that is
    /// before the end of the stream.
    fn enter(&self, i: usize, from: u64) -> Option<u64> {
        let s = self.segments.get(i)?;
        self.segment.set(i);
        if let Some(m) = &s.failure {
            let _ = io::Write::write_all(&mut io::stderr(), format!("jq: error: {m}\n").as_bytes());
            self.failures.set(self.failures.get() + 1);
        }
        let len = if s.failure.is_some() { 0 } else { s.len };
        (i + 1 < self.segments.len()).then(|| from.saturating_add(len))
    }

    /// The lines jq has fetched when its parser has read up to offset `at`.
    fn file_lines(&self, bytes: &[u8], at: usize) -> usize {
        let at = at.min(bytes.len());
        // The last byte jq needed: a value that closes itself ends at `at`;
        // a number or literal is ended by the byte after it.
        let last = match at.checked_sub(1).map(|i| bytes[i]) {
            Some(b']' | b'}' | b'"') => at - 1,
            _ => at,
        };
        let (from, mut before, mut start) = match self.counted.get() {
            c @ (from, ..) if from <= last => c,
            _ => (0, 0, 0),
        };
        for (i, &b) in bytes[from..last].iter().enumerate() {
            if b == b'\n' {
                before += 1;
                start = from + i + 1;
            }
        }
        self.counted.set((last, before, start));
        // The `fgets` chunk that holds `last`, counted from its line's start.
        let end = (start + ((last - start) / CHUNK + 1) * CHUNK).min(bytes.len());
        before + usize::from(bytes[last..end].contains(&b'\n'))
    }
}

/// A buffered reader that hands out at most one line at a time, counting the
/// lines fetched in a [`Position`], as jq's line-at-a-time reader does.
///
/// It is the only buffer between the parser and the input: it reads in
/// blocks larger than the inner reader's own buffer, which is then skipped,
/// and a byte read one at a time is served without a call.
pub struct Lines<'a, R> {
    inner: R,
    buf: Box<[u8]>,
    /// End of the line handed out, end of the data (Pathfinder: the read
    /// position is `Position::pos`).
    line_end: usize,
    filled: usize,
    at: &'a Position,
    /// Pathfinder: whether the chunk handed out ends a line.
    ends_line: bool,
    /// Pathfinder: where the file being read ends, in a stream of several
    /// files (the stream offset of `buf[0]` is `Position::base`).
    end: u64,
}

impl<'a, R> Lines<'a, R> {
    pub fn new(inner: R, at: &'a Position) -> Self {
        Self {
            inner,
            buf: vec![0; 1 << 16].into_boxed_slice(),
            line_end: 0,
            filled: 0,
            at,
            ends_line: true,
            end: 0,
        }
    }
}

impl<R: Read> Lines<'_, R> {
    /// Pathfinder: keep the bytes from the mark up to `buf[upto]` that are not
    /// kept yet, before the buffer lets go of them. A long value is copied
    /// once per buffer, not once per line.
    fn keep(&self, upto: usize) {
        if let Some((mark, ..)) = self.at.mark.get() {
            let from = mark.max(self.at.kept_end.get());
            let base = self.at.base.get();
            let upto_at = base + upto as u64;
            if from < upto_at {
                let from = (from - base) as usize;
                self.at
                    .kept
                    .borrow_mut()
                    .extend_from_slice(&self.buf[from..upto]);
                self.at.kept_end.set(upto_at);
            }
        }
    }

    /// Pathfinder: [`BufRead::fill_buf`], never inlined.
    #[inline(never)]
    fn refill(&mut self) -> io::Result<&[u8]> {
        self.fill_buf()
    }
}

impl<R> Lines<'_, R> {
    /// Pathfinder: past the end of a file, go on to the next one, as jq does
    /// when more is wanted, and say whether one was entered: its line count
    /// starts again. The first read enters the first file. Out of line, so
    /// that the per-chunk path stays as small as it was.
    #[cold]
    #[inline(never)]
    fn cross(&mut self, from: u64) -> bool {
        if self.at.lines.get().is_none() {
            self.end = self.at.enter(0, 0).unwrap_or(u64::MAX);
        }
        let mut crossed = false;
        while from >= self.end {
            let next = self.at.segment.get() + 1;
            self.end = self.at.enter(next, from).unwrap_or(u64::MAX);
            crossed = true;
        }
        crossed
    }
}

impl<R: Read> BufRead for Lines<'_, R> {
    /// The next chunk as `fgets` fetches it: up to a newline or [`CHUNK`]
    /// bytes, reading on until one of them, or the end, is there.
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        // Pathfinder: the read position lives in `Position`.
        let mut pos = self.at.pos.get();
        if pos == self.line_end {
            // Pathfinder: hand over the chunk read since the mark, if asked.
            if self.at.flush.take() {
                self.keep(self.line_end);
            }
            self.at.refills.set(self.at.refills.get() + 1);
            let mut lines = self.at.lines.get().unwrap_or(0);
            // Pathfinder: in a stream of several files, `fgets` never reads
            // across the end of one.
            let from = self.at.base.get() + pos as u64;
            if from >= self.end && self.cross(from) {
                self.at.lines_before.set(self.at.lines_before.get() + lines);
                lines = 0;
            }
            let chunk = (self.end - from).min(CHUNK as u64) as usize;
            let before = self.at.lines_before.get() + lines;
            loop {
                let rest = &self.buf[pos..self.filled];
                if let Some(i) = newline(&rest[..rest.len().min(chunk)]) {
                    self.line_end = pos + i + 1;
                    lines += 1;
                    break;
                }
                if rest.len() >= chunk {
                    self.line_end = pos + chunk;
                    break;
                }
                // Keep the partial chunk and read on behind it.
                self.keep(pos);
                self.at.base.set(self.at.base.get() + pos as u64);
                self.buf.copy_within(pos..self.filled, 0);
                (self.filled, pos) = (self.filled - pos, 0);
                self.at.pos.set(0);
                let n = match self.inner.read(&mut self.buf[self.filled..]) {
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    r => r?,
                };
                if n == 0 {
                    self.line_end = self.filled;
                    break;
                }
                self.filled += n;
            }
            self.at.lines.set(Some(lines));
            let start = self.at.base.get() + pos as u64;
            if self.ends_line {
                self.at.chunk_line.set((before, start));
            } else {
                self.at.chunk_line.set((before, self.at.chunk_line.get().1));
            }
            self.ends_line = self.buf[pos..self.line_end].last() == Some(&b'\n');
        }
        Ok(&self.buf[pos..self.line_end])
    }

    fn consume(&mut self, n: usize) {
        self.at.pos.set(self.at.pos.get() + n);
    }
}

impl<R: Read> Read for Lines<'_, R> {
    #[inline]
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        // Pathfinder: the refill stays out of line, so that this byte path
        // stays small enough to be inlined into the parser.
        if self.at.pos.get() == self.line_end && self.refill()?.is_empty() {
            return Ok(0);
        }
        let pos = self.at.pos.get();
        if let [b] = out {
            *b = self.buf[pos];
            self.at.pos.set(pos + 1);
            return Ok(1);
        }
        let n = out.len().min(self.line_end - pos);
        out[..n].copy_from_slice(&self.buf[pos..pos + n]);
        self.at.pos.set(pos + n);
        Ok(n)
    }
}

/// The first newline in `buf`, eight bytes at a time: a byte-wise search
/// measurably slowed reading a long line (no dependency may be added).
fn newline(buf: &[u8]) -> Option<usize> {
    const LO: u64 = u64::from_ne_bytes([0x01; 8]);
    const HI: u64 = u64::from_ne_bytes([0x80; 8]);
    const NL: u64 = u64::from_ne_bytes([b'\n'; 8]);
    let mut words = buf.chunks_exact(8);
    let mut at = 0;
    for w in &mut words {
        let x = u64::from_ne_bytes(w.try_into().unwrap_or_default()) ^ NL;
        if x.wrapping_sub(LO) & !x & HI != 0 {
            break;
        }
        at += 8;
    }
    buf[at..].iter().position(|&b| b == b'\n').map(|i| at + i)
}

/// jq's report of an uncaught error, newline included.
pub fn report(e: Error, location: &str) -> Vec<u8> {
    match e.into_val() {
        Val::TStr(s) | Val::BStr(s) => {
            let mut out = format!("jq: error (at {location}): ").into_bytes();
            // jq prints the message with `%s`, which stops at a NUL.
            let s = s.split(|&b| b == 0).next().unwrap_or_default();
            match core::str::from_utf8(s).ok().and_then(wording) {
                Some(text) => out.extend_from_slice(text.as_bytes()),
                None => out.extend_from_slice(s),
            }
            out.push(b'\n');
            out
        }
        v => format!("jq: error (at {location}) (not a string): {v}\n").into_bytes(),
    }
}

/// jq's wording for a message in one of jaq's templates, which carry the
/// values involved as JSON. The same rules as Pathfinder's `_pf_err`, which
/// gives a `catch` handler the same text.
fn wording(m: &str) -> Option<String> {
    let value = |s: &str| jaq_all::json::read::parse_single(s.as_bytes()).ok();
    let dumped = |v: &Val| format!("{} ({})", type_of(v), dump(v));
    if let Some(rest) = m.strip_prefix("cannot use ") {
        let (v, what) = [
            (" as iterable (array or object)", 0),
            (" as number", 1),
            (" as rangeable (array or string)", 2),
        ]
        .into_iter()
        .find_map(|(tail, what)| Some((value(rest.strip_suffix(tail)?)?, what)))?;
        return Some(match what {
            0 => format!("Cannot iterate over {}", dumped(&v)),
            1 => format!("{} number required", dumped(&v)),
            _ => format!("Cannot index {} with object", type_of(&v)),
        });
    }
    if let Some(v) = m.strip_suffix(" has no length").and_then(value) {
        return Some(format!("{} has no length", dumped(&v)));
    }
    if let Some(rest) = m.strip_prefix("cannot index ") {
        return rest.match_indices(" with ").find_map(|(i, _)| {
            let (v, k) = (value(&rest[..i])?, value(&rest[i + 6..])?);
            Some(match &k {
                Val::TStr(s) | Val::BStr(s) if s.len() < 30 => {
                    let s = String::from_utf8_lossy(s);
                    format!("Cannot index {} with string \"{s}\"", type_of(&v))
                }
                _ => format!("Cannot index {} with {}", type_of(&v), type_of(&k)),
            })
        });
    }
    let rest = m.strip_prefix("cannot calculate ")?;
    [" + ", " - ", " * ", " / ", " % "].into_iter().find_map(|op| {
        rest.match_indices(op).find_map(|(i, _)| {
            let (a, b) = (value(&rest[..i])?, value(&rest[i + 3..])?);
            let verb = match op {
                " + " => "cannot be added",
                " - " => "cannot be subtracted",
                " * " => "cannot be multiplied",
                " / " => "cannot be divided",
                _ => "cannot be divided (remainder)",
            };
            let zero = matches!((&a, &b), (Val::Num(_), Val::Num(_)))
                && b > Val::from(-1_isize)
                && b < Val::from(1_isize)
                && matches!(op, " / " | " % ");
            let why = if zero { " because the divisor is zero" } else { "" };
            Some(format!("{} and {} {verb}{why}", dumped(&a), dumped(&b)))
        })
    })
}

/// jq's name for the type of `v`.
fn type_of(v: &Val) -> &'static str {
    match v {
        Val::Null => "null",
        Val::Bool(_) => "boolean",
        Val::Num(_) => "number",
        Val::BStr(_) | Val::TStr(_) => "string",
        Val::Arr(_) => "array",
        Val::Obj(_) => "object",
    }
}

/// jq 1.8.1's value in an error message: its JSON, cut to 11 bytes and `...`
/// when longer than 14, never splitting a character.
fn dump(v: &Val) -> String {
    let s = v.to_string();
    if s.len() <= 14 {
        return s;
    }
    let mut end = 0;
    for (i, c) in s.char_indices() {
        if i + c.len_utf8() > 11 {
            break;
        }
        end = i + c.len_utf8();
    }
    format!("{}...", &s[..end])
}

#[cfg(test)]
mod tests {
    use super::{Lines, Position, Segment};
    use std::io::BufRead;

    fn segments(s: &str) -> Vec<Segment> {
        Segment::parse(s.as_bytes()).unwrap()
    }

    /// The chunks a stream of several files is fetched in, each with the
    /// location after fetching it.
    fn chunks(input: &[u8], desc: &str) -> Vec<(String, String)> {
        let pos = Position::with_segments(segments(desc));
        let mut lines = Lines::new(input, &pos);
        let mut out = Vec::new();
        loop {
            let chunk = lines.fill_buf().unwrap().to_vec();
            if chunk.is_empty() {
                return out;
            }
            lines.consume(chunk.len());
            let chunk = String::from_utf8(chunk).unwrap();
            out.push((chunk, pos.location("<stdin>", None)));
        }
    }

    fn pairs(want: &[(&str, &str)]) -> Vec<(String, String)> {
        want.iter()
            .map(|(c, l)| (c.to_string(), l.to_string()))
            .collect()
    }

    #[test]
    fn parses_the_description() {
        let s = segments("4:6:a.json0:0:2:b:7:no such0:7:<stdin>0:");
        assert_eq!(s.len(), 3);
        assert_eq!(
            (s[0].name.as_str(), s[0].len, s[0].failure.as_deref()),
            ("a.json", 4, None)
        );
        assert_eq!(
            (s[1].name.as_str(), s[1].failure.as_deref()),
            ("b:", Some("no such"))
        );
        assert_eq!(s[2].name, "<stdin>");
        assert!(Segment::parse(b"4:6:a.js").is_none());
    }

    #[test]
    fn files_are_chunk_boundaries_and_restart_the_count() {
        let got = chunks(b"1\n2\n1 2\"z\"", "4:1:a0:3:1:b0:0:1:c0:");
        let want = [
            ("1\n", "a:1"),
            ("2\n", "a:2"),
            ("1 2", "b:0"),
            ("\"z\"", "c:0"),
        ];
        assert_eq!(got, pairs(&want));
    }

    #[test]
    fn empty_and_unreadable_files_are_passed_through() {
        let desc = "2:1:a0:0:1:e0:0:1:m3:err";
        assert_eq!(chunks(b"12", desc), pairs(&[("12", "a:0")]));
        let pos = Position::with_segments(segments(desc));
        let mut lines = Lines::new(&b"12"[..], &pos);
        let n = lines.fill_buf().unwrap().len();
        lines.consume(n);
        assert!(lines.fill_buf().unwrap().is_empty());
        assert_eq!(pos.location("<stdin>", None), "m:0");
        assert!(pos.failed());
    }

    #[test]
    fn a_single_stream_is_unchanged() {
        let got = chunks(b"1\n2", "");
        assert_eq!(got, pairs(&[("1\n", "<stdin>:1"), ("2", "<stdin>:1")]));
    }
}
