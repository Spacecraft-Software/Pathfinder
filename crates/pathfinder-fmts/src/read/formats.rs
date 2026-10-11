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
            let mut buf = Vec::new();
            read.read_to_end(&mut buf).map(|_| Val::utf8_str(buf))
        })),
        Format::Raw => Box::new(read.byte_lines().map(|r| r.map(Val::utf8_str))),
        Format::Raw0 => collect_if(slurp, read.byte_records(0).map(|r| r.map(Val::utf8_str))),
        Format::Json => collect_if(slurp, json::read_many(read)),
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
