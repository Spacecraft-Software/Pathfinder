//! Read values as JSON or raw text.
pub use jaq_json::read as json;

mod formats;
pub use formats::*;

use bytes::Bytes;
use jaq_json::Val;
use std::path::Path;

type Result<T, E = std::io::Error> = core::result::Result<T, E>;

/// Load a file into memory.
///
/// Pathfinder (TODO T-005): jaq memory-maps the file, which needs `unsafe`
/// and is unsound if another process truncates or rewrites the file while
/// it is mapped. A plain read costs about the same here (crates/UPSTREAM.md).
pub fn load_file(path: impl AsRef<Path>) -> Result<Bytes> {
    Ok(Bytes::from(std::fs::read(path)?))
}

/// Read JSON values in a file to an array.
pub fn json_array(path: impl AsRef<Path>) -> Result<Val> {
    json::parse_many(&load_file(path.as_ref())?)
        .map(crate::map_invalid_data)
        .collect()
}

/// Collect iterator into a single array if `slurp`, else return iterator.
pub fn collect_if<'a, T: FromIterator<T> + 'a, E: 'a>(
    slurp: bool,
    iter: impl Iterator<Item = Result<T, E>> + 'a,
) -> Box<dyn Iterator<Item = Result<T, E>> + 'a> {
    if slurp {
        // Pathfinder: slurped only when asked for, so the reader is where jq's
        // is when an input file turns out to be unreadable.
        Box::new(core::iter::once_with(move || iter.collect()))
    } else {
        Box::new(iter)
    }
}
