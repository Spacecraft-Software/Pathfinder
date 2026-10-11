//! Write values as JSON or raw text, and `@csv`/`@tsv` rows.
pub mod tabular;
pub use jaq_json::write as json;

mod formats;
mod funs;
pub use formats::write;
pub use funs::funs;

use std::io::{self, IsTerminal, Write};

/// Write options.
#[derive(Default)]
pub struct Writer {
    /// output format
    pub format: crate::Format,
    /// pretty printer
    pub pp: json::Pp,
    /// concatenate outputs without newline
    pub join: bool,
}

/// Buffer writes if stdout is terminal, else just lock stdout.
pub fn with_stdout<T>(stdout: &mut io::Stdout, f: impl FnOnce(&mut dyn Write) -> T) -> T {
    if stdout.is_terminal() {
        f(&mut stdout.lock())
    } else {
        f(&mut io::BufWriter::new(stdout.lock()))
    }
}
