// SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Fuzz evaluation: a fuzzed filter over fuzzed JSON, through the engine
//! crates, as the shim will run it in process (M19).
//!
//! The input is `filter NUL json`. The filter goes through
//! `program::assemble` first, so the engine sees what the shim hands it in
//! production (prelude, rewrites, `$__loc__`); a filter jq would reject at
//! compile time is dropped there. The JSON is read with the jq-wording reader
//! (`parse_many`), and every output is serialised.
//!
//! # Termination
//!
//! jaq has no step limit, so termination is bounded three ways:
//!
//! - **Rejected text** ([`unbounded`]): constructs that can run or recurse
//!   without end on finite input, and those with side effects that would end
//!   or flood the fuzzer process. Filters containing them are skipped.
//! - **Caps**: at most [`MAX_INPUTS`] input values and [`MAX_OUTPUTS`] outputs
//!   per run; reaching either stops the run.
//! - **libFuzzer's guards**: what the text filter cannot see (`range(0; 1; 0)`
//!   collected into an array, string repetition by a computed count) is cut
//!   off by `-timeout` and `-rss_limit_mb`. Run this target in fork mode with
//!   `-ignore_timeouts=1 -ignore_ooms=1` (AGENTS.md, "Fuzzing"): a timeout or
//!   OOM here is a resource bound, not a finding, unless a reproducer shows
//!   the work was bounded.

#![no_main]

use jaq_all::data::{self, Runner};
use jaq_all::fmts;
use jaq_all::jaq_core::Vars;
use jaq_all::json::read;
use libfuzzer_sys::fuzz_target;
use pathfinder_fuzz::{prelude, program};

/// Input values read per run.
const MAX_INPUTS: usize = 64;

/// Outputs taken per run before it is stopped.
const MAX_OUTPUTS: usize = 256;

/// Words whose presence skips the filter.
///
/// - `def`: a user definition can recurse without end, and jaq recurses on
///   the native stack, so the fuzzer would report a stack overflow that is
///   the engine's documented lack of a depth limit, not a bug.
/// - `repeat`, `while`, `until`, `recurse(`: unbounded on any input.
///   (`recurse` without arguments and `..` stay: they are bounded by the
///   input.)
/// - `halt`: `halt` and `halt_error` end the process (`unwrap_valr`'s
///   `process::exit`); libFuzzer would report every one as a crash.
/// - `debug`, `stderr`: write to stderr on every run, burying libFuzzer's
///   own log.
const REJECTED: &[&str] = &[
    "def", "repeat", "while", "until", "recurse(", "halt", "debug", "stderr",
];

/// Whether `filter` may run without end or end the process; see [`REJECTED`].
///
/// Number literals are capped too: a run of four or more digits, or an
/// exponent, makes `range`, `limit` and string repetition cheap ways to
/// exhaust time or memory.
fn unbounded(filter: &str) -> bool {
    if REJECTED.iter().any(|w| filter.contains(w)) {
        return true;
    }
    let bytes = filter.as_bytes();
    let mut run = 0usize;
    for (i, &b) in bytes.iter().enumerate() {
        if b.is_ascii_digit() {
            run += 1;
            if run >= 4 {
                return true;
            }
        } else {
            let exponent = matches!(b, b'e' | b'E')
                && run > 0
                && bytes
                    .get(i + 1)
                    .is_some_and(|&n| n.is_ascii_digit() || n == b'+' || n == b'-');
            if exponent {
                return true;
            }
            run = 0;
        }
    }
    false
}

/// Why a run stopped early. Carries nothing: only the stop matters.
struct Stop;

fuzz_target!(|data: &[u8]| {
    let (filter, json) = match data.iter().position(|&b| b == 0) {
        Some(at) => (&data[..at], &data[at + 1..]),
        None => (data, &b"null"[..]),
    };
    let Ok(filter) = std::str::from_utf8(filter) else {
        return;
    };
    if unbounded(filter) {
        return;
    }

    let assembly = program::assemble(filter, &prelude::Context::default(), None, "<top-level>");
    if assembly.compile_error.is_some() {
        return;
    }
    let text = assembly.text;
    let Ok(compiled) = data::compile(&text) else {
        return;
    };

    let runner = Runner::default();
    let inputs = read::parse_many(json).take(MAX_INPUTS);
    let mut outputs = 0usize;
    let mut sink = Vec::new();
    let _ = data::run(
        &runner,
        &compiled,
        Vars::new([]),
        inputs,
        |_| Stop,
        |y| {
            // An error is an output too; `halt` cannot get here (rejected).
            if let Ok(v) = y {
                sink.clear();
                let _ = fmts::write::write(&mut sink, &runner.writer, &v);
            }
            outputs += 1;
            if outputs >= MAX_OUTPUTS {
                Err(Stop)
            } else {
                Ok(())
            }
        },
    );
});

// Rust guideline compliant 2026-05-18
