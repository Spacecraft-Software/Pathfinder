// SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Fuzz jq's command-line grammar (`jqargs`).
//!
//! The input is argv without `argv[0]`, one entry per NUL-separated field. The
//! bytes are taken as they come, non-UTF-8 included: `jqargs::parse` reads raw
//! `OsString`s, as jq's `main` reads `char *`s.

#![no_main]

use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt as _;

use libfuzzer_sys::fuzz_target;
use pathfinder_fuzz::jqargs;

fuzz_target!(|data: &[u8]| {
    let argv: Vec<OsString> = data
        .split(|&b| b == 0)
        .map(|field| OsString::from_vec(field.to_vec()))
        .collect();

    match jqargs::parse(&argv) {
        Ok(args) => drop(args),
        Err(e) => drop(e.line("jq")),
    }
    for i in jqargs::option_positions(&argv) {
        // Every position names an entry of argv.
        let _ = jqargs::stop_at(&argv[i]);
    }
});

// Rust guideline compliant 2026-05-18
