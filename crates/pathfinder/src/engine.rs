// SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Running a plan on the engine, in process.
//!
//! The engine is Pathfinder's crates, linked into this binary: nothing is
//! spawned. Until 0.2.0 a jaq binary could stand in for it
//! (`PATHFINDER_ENGINE=jaq`); that engine is gone (PLAN P-078), and the
//! variable is still read so that a script that still asks for it fails
//! instead of silently running something else.

use std::io::{self, IsTerminal as _, Read, Write};

use crate::inputs;
use crate::post::{self, Framer};
use crate::translate::Plan;

/// The variable that once chose the engine.
pub const ENGINE_ENV: &str = "PATHFINDER_ENGINE";

/// The engine's name, as `--version` and `--explain` report it.
pub const NAME: &str = "native";

/// Check [`ENGINE_ENV`]: unset, empty or `native` is the one engine.
///
/// # Errors
///
/// Any other value, as the message to report.
pub fn check() -> Result<(), String> {
    match std::env::var(ENGINE_ENV) {
        Ok(v) if !v.is_empty() && v != NAME => Err(format!(
            "{ENGINE_ENV} is {v:?}; Pathfinder has one engine, \"native\", since 0.2.0 \
             (the jaq engine was removed): unset {ENGINE_ENV}"
        )),
        _ => Ok(()),
    }
}

/// Run the plan on the native engine and return the exit status.
///
/// Several input files are read as one stream, described by
/// [`jaq::Segment`]s; `--seq` input loses its separators on the way in; `-a`
/// and `--seq` output is framed on the way out by [`Framer`].
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
