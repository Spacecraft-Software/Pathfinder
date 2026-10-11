// SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Multi-call dispatch on `argv[0]`.
//!
//! The binary is installed as `pathfinder` with a `jq` symlink beside it, the
//! same busybox-style arrangement `rust-os-prober` uses for its `os-prober`
//! alias. Both names run the identical translation — the name only decides how
//! diagnostics introduce themselves, and whether Pathfinder's own
//! non-jq subcommands are reachable.
//!
//! Keeping the surfaces identical matters: a bug reproduced as `pathfinder` is
//! the same bug the `jq` symlink has, so there is never a "but it works when I
//! call it directly" class of report.

use std::ffi::OsString;
use std::path::Path;

/// Which name the binary was invoked under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvokedAs {
    /// Invoked through the `jq` symlink: pure drop-in mode.
    Jq,
    /// Invoked under its own name: drop-in mode plus Pathfinder's own verbs.
    Pathfinder,
}

impl InvokedAs {
    /// The name to print in diagnostics.
    pub const fn prog(self) -> &'static str {
        match self {
            Self::Jq => "jq",
            Self::Pathfinder => "pathfinder",
        }
    }

    /// Whether Pathfinder's own subcommands (`--install-shim`, `--explain`) are
    /// reachable.
    ///
    /// They are hidden behind the real name so that the `jq` surface stays
    /// exactly jq's: a script passing `--explain` through to what it believes is
    /// jq must get jq's "Unknown option" error, not a Pathfinder feature.
    pub const fn allows_own_verbs(self) -> bool {
        matches!(self, Self::Pathfinder)
    }
}

/// Detect the invocation name from `argv[0]`'s base name.
///
/// Anything that is not exactly `jq` is treated as the native name, including a
/// missing or non-UTF-8 `argv[0]`; the native name is the safe default because
/// it is the strictly larger surface.
pub fn detect(argv: &[OsString]) -> InvokedAs {
    let name = argv
        .first()
        .and_then(|a| Path::new(a).file_name())
        .and_then(|n| n.to_str());
    match name {
        Some("jq") => InvokedAs::Jq,
        _ => InvokedAs::Pathfinder,
    }
}

#[cfg(test)]
mod tests {
    use super::{InvokedAs, detect};
    use std::ffi::OsString;

    fn argv(parts: &[&str]) -> Vec<OsString> {
        parts.iter().map(OsString::from).collect()
    }

    #[test]
    fn detects_the_shim_name_by_basename() {
        assert_eq!(detect(&argv(&["/usr/bin/jq"])), InvokedAs::Jq);
        assert_eq!(detect(&argv(&["jq", "-r", ".a"])), InvokedAs::Jq);
    }

    #[test]
    fn anything_else_is_the_native_name() {
        assert_eq!(
            detect(&argv(&["/nix/store/x/bin/pathfinder"])),
            InvokedAs::Pathfinder
        );
        assert_eq!(detect(&argv(&[])), InvokedAs::Pathfinder);
        // A `jq-1.7` symlink is not `jq`, and must not silently claim to be.
        assert_eq!(detect(&argv(&["jq-1.7"])), InvokedAs::Pathfinder);
    }

    #[test]
    fn only_the_native_name_exposes_pathfinder_verbs() {
        assert!(!InvokedAs::Jq.allows_own_verbs());
        assert!(InvokedAs::Pathfinder.allows_own_verbs());
    }
}
