// SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Output mode for Pathfinder's own verbs: human text or one JSON document.
//!
//! Decided by the Spacecraft CLI Standard's cascade (§5), first match wins:
//! an explicit `--json`, then an agent environment (`AI_AGENT`, `AGENT`), then
//! a truthy `CI`, then whether stdout is a terminal.
//!
//! This is consulted **only** under the native name, and only for a native
//! verb or `--version`. Under the name `jq` nothing here runs, not even the
//! environment reads: an agent calling `jq` must get jq's bytes, and the
//! differential suite runs under a live agent environment to prove it.

use std::env;
use std::ffi::OsString;
use std::io::{self, IsTerminal as _};

/// How a native verb reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The text a person reads at a terminal.
    Human,
    /// One compact JSON document per report.
    Json,
}

/// Decide the mode from the process environment and stdout.
pub fn detect(json_flag: bool) -> Mode {
    cascade(
        json_flag,
        |name| env::var_os(name),
        || io::stdout().is_terminal(),
    )
}

/// The cascade itself, with the environment and the TTY test injected.
fn cascade(
    json_flag: bool,
    var: impl Fn(&str) -> Option<OsString>,
    stdout_is_tty: impl FnOnce() -> bool,
) -> Mode {
    if json_flag {
        return Mode::Json;
    }
    // Presence, not a value: agents set descriptive strings such as
    // `claude-code_2-1-296_agent`, so comparing against "1" would miss them.
    let set = |name: &str| var(name).is_some_and(|v| !v.is_empty());
    if set("AI_AGENT") || set("AGENT") {
        return Mode::Json;
    }
    // `CI=false` and `CI=0` are how a developer says "not CI" explicitly.
    if var("CI").is_some_and(|v| !v.is_empty() && v != "false" && v != "0") {
        return Mode::Json;
    }
    if stdout_is_tty() {
        Mode::Human
    } else {
        Mode::Json
    }
}

#[cfg(test)]
mod tests {
    use super::{Mode, cascade};
    use std::ffi::OsString;

    fn with(vars: &[(&str, &str)], json_flag: bool, tty: bool) -> Mode {
        cascade(
            json_flag,
            |name| {
                vars.iter()
                    .find(|(k, _)| *k == name)
                    .map(|(_, v)| OsString::from(v))
            },
            || tty,
        )
    }

    #[test]
    fn an_explicit_flag_wins() {
        assert_eq!(with(&[], true, true), Mode::Json);
        assert_eq!(with(&[("CI", "false")], true, true), Mode::Json);
    }

    #[test]
    fn any_non_empty_agent_value_means_json() {
        let agent = "claude-code_2-1-296_agent";
        assert_eq!(with(&[("AI_AGENT", agent)], false, true), Mode::Json);
        assert_eq!(with(&[("AGENT", "1")], false, true), Mode::Json);
        assert_eq!(with(&[("AI_AGENT", "")], false, true), Mode::Human);
    }

    #[test]
    fn ci_counts_only_when_truthy() {
        assert_eq!(with(&[("CI", "true")], false, true), Mode::Json);
        assert_eq!(with(&[("CI", "1")], false, true), Mode::Json);
        assert_eq!(with(&[("CI", "false")], false, true), Mode::Human);
        assert_eq!(with(&[("CI", "0")], false, true), Mode::Human);
        assert_eq!(with(&[("CI", "")], false, true), Mode::Human);
    }

    #[test]
    fn otherwise_the_terminal_decides() {
        assert_eq!(with(&[], false, true), Mode::Human);
        assert_eq!(with(&[], false, false), Mode::Json);
        assert_eq!(with(&[("CI", "false")], false, false), Mode::Json);
    }
}
