use super::{collect_if, json};
use crate::{map_invalid_data, Format};
use bytes::Bytes;
use jaq_core::box_iter::{box_once, BoxIter};
use jaq_json::Val;
use std::io;

type Vals<'a> = BoxIter<'a, io::Result<Val>>;

/// Read values from [`io::BufRead`], depending on format.
pub fn read<'a>(fmt: Format, read: impl io::BufRead + 'a, slurp: bool) -> Vals<'a> {
    use bstr::io::BufReadExt;
    let mut read = read;
    match fmt {
        // Pathfinder: read only when asked for, as `collect_if` slurps.
        Format::Raw if slurp => Box::new(core::iter::once_with(move || {
            let mut text = String::new();
            while raw_chunk(&mut read, &mut text)? {}
            Ok(Val::utf8_str(text))
        })),
        Format::Raw => Box::new(core::iter::from_fn(move || raw_line(&mut read).transpose())),
        Format::Raw0 => collect_if(slurp, read.byte_records(0).map(|r| r.map(Val::utf8_str))),
        Format::Json => collect_if(slurp, json::read_many(read)),
    }
}

/// Pathfinder: append the next piece of raw input to `text` as jq reads
/// it, one `fgets` chunk made a string on its own (`jq_util_input_next_input`'s
/// `jv_string_sized`), so a character split across two chunks becomes
/// U+FFFD as in jq. `read` hands out chunks as `fgets` fetches them
/// (`jaq::jq::Lines`); `false` at the end.
fn raw_chunk(read: &mut impl io::BufRead, text: &mut String) -> io::Result<bool> {
    let chunk = read.fill_buf()?;
    let len = chunk.len();
    jaq_std::push_jq_lossy_utf8(text, chunk);
    read.consume(len);
    Ok(len > 0)
}

/// Pathfinder: the next line of raw input, as jq reads it: the chunks up to
/// a newline, each made a string on its own, without the newline (a `\r`
/// before it stays). A last line without a newline is a line; `None` at the
/// end.
fn raw_line(read: &mut impl io::BufRead) -> io::Result<Option<Val>> {
    use bstr::ByteSlice;
    let mut line: Option<String> = None;
    loop {
        let chunk = read.fill_buf()?;
        // `jq::Lines` ends a chunk at its first newline; another reader may
        // not.
        let len = chunk.find_byte(b'\n').map_or(chunk.len(), |i| i + 1);
        let Some(&last) = chunk[..len].last() else {
            return Ok(line.map(Val::utf8_str));
        };
        let body = if last == b'\n' {
            &chunk[..len - 1]
        } else {
            &chunk[..len]
        };
        jaq_std::push_jq_lossy_utf8(line.get_or_insert_with(String::new), body);
        read.consume(len);
        if last == b'\n' {
            return Ok(line.map(Val::utf8_str));
        }
    }
}

/// Parse values from [`Bytes`], depending on format.
pub fn parse<'a>(fmt: Format, bytes: &'a Bytes, slurp: bool) -> Vals<'a> {
    use bstr::ByteSlice;
    let nul_sep = |s: &'a [u8]| s.strip_suffix(b"\0").unwrap_or(bytes).split_str("\0");
    let slice_to_str = |s| Ok(Val::utf8_str(bytes.slice_ref(s)));
    match fmt {
        Format::Raw if slurp => box_once(Ok(Val::utf8_str(bytes.clone()))),
        Format::Raw => Box::new(bytes.lines().map(slice_to_str)),
        Format::Raw0 => collect_if(slurp, nul_sep(bytes).map(slice_to_str)),
        Format::Json => collect_if(slurp, json::parse_many(bytes).map(map_invalid_data)),
    }
}
