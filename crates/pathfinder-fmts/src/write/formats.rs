use super::Writer;
use crate::Format;
use jaq_json::Val;
use std::io::{self, Write};

type Result<T = (), E = io::Error> = core::result::Result<T, E>;

/// Write value.
pub fn write(w: &mut dyn Write, writer: &Writer, val: &Val) -> Result {
    let Writer { format, pp, join } = writer;

    match (val, format) {
        (Val::BStr(b) | Val::TStr(b), Format::Raw0) if b.contains(&b'\0') => {
            let nul_err = "cannot dump a string containing NUL with `--to raw0` or `--raw-output0`";
            return Err(io::Error::new(io::ErrorKind::InvalidData, nul_err));
        }
        (Val::BStr(b) | Val::TStr(b), Format::Raw | Format::Raw0) => w.write_all(b)?,
        (_, Format::Json | Format::Raw | Format::Raw0) => jaq_json::write::write(w, pp, 0, val)?,
    };

    w.write_all(match format {
        Format::Raw0 => b"\0",
        _ if *join => b"",
        _ => b"\n",
    })?;

    // when running `jaq -jn '"prompt> " | (., input)'`,
    // this flush is necessary to make "prompt> " appear first
    w.flush()
}
