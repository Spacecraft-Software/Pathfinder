// SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Diagnostics, in jq's own shape.
//!
//! Pathfinder stands in for `jq`, so its failures have to look like jq's
//! failures: same stream (stderr), same exit codes, same trailing "use --help"
//! couplet. A script that branches on `$?` must not be able to tell the
//! difference. Where Pathfinder reports something jq never would — an
//! unsupported feature — it says so in its own name so the reader knows which
//! program is talking.

/// jq's exit code for any command-line usage failure (`die()` in `jq/src/main.c`).
pub const EXIT_USAGE: i32 = 2;

/// Print a jq-shaped usage error and terminate with jq's usage exit code.
///
/// `prog` is `argv[0]`'s base name, so the message reads `jq: …` when invoked
/// through the shim symlink and `pathfinder: …` when invoked directly.
pub fn die(prog: &str, message: &str) -> ! {
    eprintln!("{prog}: {message}");
    eprintln!("Use {prog} --help for help with command-line options,");
    eprintln!("or see the jq manpage, or online docs at https://jqlang.org");
    std::process::exit(EXIT_USAGE);
}

/// Report a jq feature that jaq cannot provide, naming the feature and the
/// escape hatch.
///
/// Deliberately not silent: the alternative is producing an answer that differs
/// from jq's without telling anyone, which is the one failure mode a
/// compatibility shim must never have.
pub fn unsupported(prog: &str, feature: &str, detail: &str) -> ! {
    eprintln!("{prog}: {feature} is not supported by jaq, so Pathfinder cannot emulate it.");
    eprintln!("{prog}: {detail}");
    eprintln!("{prog}: see doc/DIVERGENCES.md in the Pathfinder source tree.");
    std::process::exit(EXIT_USAGE);
}

/// Print a non-fatal warning attributed to Pathfinder rather than to jq.
pub fn warn(prog: &str, message: &str) {
    eprintln!("{prog}: warning: {message}");
}
