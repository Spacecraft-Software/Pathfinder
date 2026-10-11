//! Multi-format support for jaq.
//!
//! This crate contains functions to load and write the formats jq reads and
//! writes: JSON and raw text.
//!
//! Pathfinder (TODO T-004): jaq's CBOR, YAML, TOML, XML and CSV/TSV readers
//! and writers are removed, with the `from…`/`to…` filters for them. `@csv`
//! and `@tsv`, which jq defines, stay.
#![warn(missing_docs)]
#![expect(
    clippy::unnecessary_semicolon,
    reason = "jaq 3.1.1 as imported (crates/UPSTREAM.md): kept textually close to upstream so later upstream fixes port by diff (TODO T-020); restyling it would bury the patch set"
)]
#![expect(
    clippy::must_use_candidate,
    missing_debug_implementations,
    reason = "jaq 3.1.1 as imported (crates/UPSTREAM.md): changing the public signatures, derives or attributes of the engine's API ripples through every crate, and M19 reshapes that API"
)]

extern crate alloc;
extern crate std;

pub mod read;
pub mod write;

use jaq_core::native::{run, Fun};

/// Serialisation filters: `@csv` and `@tsv`.
pub fn funs<D: for<'a> jaq_core::DataT<V<'a> = jaq_json::Val>>() -> impl Iterator<Item = Fun<D>> {
    write::funs::<D>().into_vec().into_iter().map(run::<D>)
}

/// Input/Output format.
#[non_exhaustive]
#[derive(Copy, Clone, Debug, Default)]
pub enum Format {
    /// Raw text string
    ///
    /// When the option `--slurp` is used additionally,
    /// then the whole input is read into a single string.
    Raw,
    /// Zero-terminated text string
    Raw0,
    /// JavaScript Object Notation
    #[default]
    Json,
}

impl Format {
    /// List of all currently supported formats.
    pub const ALL: &str = "raw, raw0, json";

    /// Parse a format name.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "raw" => Some(Format::Raw),
            "raw0" => Some(Format::Raw0),
            "json" => Some(Format::Json),
            _ => None,
        }
    }
}

/// Dynamic & thread-safe [`std::error::Error`].
type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Create an invalid data I/O error.
fn invalid_data(e: impl Into<BoxError>) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, e)
}

fn map_invalid_data<T>(r: Result<T, impl Into<BoxError>>) -> std::io::Result<T> {
    r.map_err(invalid_data)
}
