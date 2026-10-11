// SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The engine a filter runs on, and running it in process.
//!
//! Two engines answer a `jq` call while the fork is under way (PRD R-014):
//!
//! - **`native`** — Pathfinder's engine crates, linked into this binary and
//!   run in process ([`run`]). Nothing is spawned.
//! - **`jaq`** — a jaq binary, `exec`ed or spawned (`main.rs`). The packaged
//!   jaq carries the same patch set as the engine crates, so the two agree;
//!   CI runs both suites on both until M24 retires this one.
//!
//! `PATHFINDER_ENGINE` chooses; unset, the build's default applies, which a
//! packager sets with `PATHFINDER_DEFAULT_ENGINE` at build time and which is
//! otherwise `native`.

use std::io::{self, IsTerminal as _, Read, Write};

use crate::inputs;
use crate::post::{self, Framer};
use crate::translate::Plan;

/// The runtime override.
pub const ENGINE_ENV: &str = "PATHFINDER_ENGINE";

/// The engine a packager chose at build time, if any.
const BUILT_IN: Option<&str> = option_env!("PATHFINDER_DEFAULT_ENGINE");

/// Which engine runs the filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    /// The engine crates, in process.
    Native,
    /// A jaq binary, found by `main.rs`'s `jaq_binary`.
    Jaq,
}

impl Engine {
    /// The name `PATHFINDER_ENGINE` takes and `--version` reports.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Jaq => "jaq",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        match name {
            "native" => Some(Self::Native),
            "jaq" => Some(Self::Jaq),
            _ => None,
        }
    }
}

/// The engine selected, and what selected it: [`ENGINE_ENV`], the build's
/// default (`built-in`), or Pathfinder's (`default`).
///
/// # Errors
///
/// A value of [`ENGINE_ENV`] (or of the build's default) that names no
/// engine, as the message to report.
pub fn selected() -> Result<(Engine, &'static str), String> {
    let (value, source) = match std::env::var(ENGINE_ENV) {
        Ok(v) if !v.is_empty() => (v, ENGINE_ENV),
        _ => match BUILT_IN {
            Some(v) => (v.to_owned(), "built-in"),
            None => return Ok((Engine::Native, "default")),
        },
    };
    Engine::parse(&value)
        .map(|e| (e, source))
        .ok_or_else(|| format!("{source} is {value:?}; it must be \"native\" or \"jaq\""))
}

/// Run the plan on the native engine and return the exit status.
///
/// What the jaq path does around the binary is done here around the driver:
/// several input files are read as one stream, described by
/// [`jaq::Segment`]s instead of `PATHFINDER_INPUTS`; `--seq` input loses its
/// separators on the way in; `-a` and `--seq` output is framed on the way out
/// by the same [`Framer`] that reshapes a jaq binary's stdout.
pub fn run(plan: &Plan) -> i32 {
    jaq::init_logging();
    let strip = plan.strip_input_rs;
    let (input, segments): (Box<dyn Read>, Vec<jaq::Segment>) = match &plan.concat_inputs {
        Some(files) => {
            let inputs = inputs::open(files, strip);
            let segments = inputs::segments(&inputs);
            (Box::new(inputs::reader(inputs, strip)), segments)
        }
        None if strip => (Box::new(post::StripRs(io::stdin().lock())), Vec::new()),
        None => (Box::new(io::stdin().lock()), Vec::new()),
    };

    // jaq's choice: unbuffered to a terminal, so each value shows at once,
    // block-buffered otherwise.
    let stdout = io::stdout();
    let mut out: Box<dyn Write> = if stdout.is_terminal() {
        Box::new(stdout.lock())
    } else {
        Box::new(io::BufWriter::new(stdout.lock()))
    };
    let code = if plan.post.is_identity() {
        let code = jaq::run(&plan.cli, input, segments, &mut out);
        // Like jaq's: a failure on the last flush, a closed pipe included,
        // changes nothing.
        let _ = out.flush();
        code
    } else {
        let mut framer = Framer::new(out, plan.post);
        let code = jaq::run(&plan.cli, input, segments, &mut framer);
        let _ = framer.finish();
        code
    };
    i32::from(code)
}
