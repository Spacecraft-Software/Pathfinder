// SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Fuzz the shim's jq front end: lexer, parser, printer, rewriter, compile
//! checks, and the program assembly that runs them all.
//!
//! The input is a jq program. Not UTF-8 means not a program: argv reaches the
//! assembler only as a `&str`.

#![no_main]

use libfuzzer_sys::fuzz_target;
use pathfinder_fuzz::{prelude, program, syntax};

fuzz_target!(|data: &[u8]| {
    let Ok(src) = std::str::from_utf8(data) else {
        return;
    };

    drop(syntax::lex::lex(src));
    if let Ok(tree) = syntax::parse::parse(src) {
        drop(syntax::print::full(&tree, src));
    }

    // The path the shim takes for every filter: scan, compile checks,
    // `$__loc__`, rewrite, prelude.
    drop(program::assemble(
        src,
        &prelude::Context::default(),
        None,
        "<top-level>",
    ));
});

// Rust guideline compliant 2026-05-18
