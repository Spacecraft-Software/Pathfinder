// SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Pathfinder's own verbs, reachable only under its own name.
//!
//! These are deliberately unavailable through the `jq` symlink. A script that
//! passes `--explain` to what it believes is jq must get jq's `Unknown option`
//! error, not a Pathfinder feature — the drop-in surface has to *be* jq's
//! surface, or it is not a drop-in.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::jqargs::JqArgs;
use crate::translate::Plan;

/// Show what Pathfinder would run, without running it.
///
/// This is the debugging entry point: when a filter behaves unexpectedly, the
/// first question is always "what did jaq actually receive", and guessing at it
/// from the source is how subtle rewriting bugs survive.
pub fn explain(args: &JqArgs, plan: &Plan) {
    println!("jaq argv:");
    for a in &plan.argv {
        println!("  {}", a.to_string_lossy());
    }

    match &plan.assembly {
        Some(a) => {
            println!("\nprogram: rewritten");
            if !a.injected.is_empty() {
                println!("  injected: {}", a.injected.join(", "));
            }
            if !a.shadowed.is_empty() {
                println!(
                    "  shadowed by your own definitions (yours wins): {}",
                    a.shadowed.join(", ")
                );
            }
            println!("  text: {}", a.text);
        }
        None => println!("\nprogram: passed through unchanged"),
    }

    println!(
        "\nexecution: {}",
        if needs_pipe(plan) {
            "spawn + pipe"
        } else {
            "exec (no extra process)"
        }
    );
    if plan.post.ascii {
        println!("  stdout: escape non-ASCII (-a)");
    }
    if plan.post.seq {
        println!("  stdout: prefix each value with RS (--seq)");
    }
    if plan.strip_input_rs {
        println!("  stdin:  strip RS separators (--seq)");
    }
    if let Some(files) = &plan.concat_inputs {
        println!(
            "  stdin:  concatenate {} input files, because jaq would otherwise run the \
             program once per file",
            files.len()
        );
    }

    // Assignment operators are the one common divergence Pathfinder cannot
    // repair, so say so where the user is already looking.
    if uses_assignment(args) {
        println!(
            "\nnote: this filter assigns to a path. jaq does not create missing \
             containers the way jq does, so `null | .a.b = 1` errors instead of \
             producing {{\"a\":{{\"b\":1}}}}. Calls to `setpath` are repaired; the \
             `=`, `|=` and `+=` operators cannot be. See doc/DIVERGENCES.md."
        );
    }
}

/// Whether the plan gives up the `exec` fast path.
fn needs_pipe(plan: &Plan) -> bool {
    !plan.post.is_identity() || plan.strip_input_rs || plan.concat_inputs.is_some()
}

/// Crude check for an assignment operator in the filter, used only to decide
/// whether to print a warning.
fn uses_assignment(args: &JqArgs) -> bool {
    args.program
        .as_ref()
        .and_then(|p| p.to_str())
        .is_some_and(|p| {
            p.contains("|=")
                || p.contains("+=")
                || p.contains("-=")
                || p.contains("//=")
                || p.contains('=')
        })
}

/// Result of installing the `jq` shim symlink.
#[derive(Debug)]
pub enum Installed {
    Created(PathBuf),
    Refused(String),
}

/// Create the `jq` symlink in `dir`, pointing at this binary.
///
/// Refuses to clobber an existing `jq` unless `force` is set: silently replacing
/// the system's JSON processor is not something a tool should do on a user's
/// behalf, and the failure mode if Pathfinder is wrong about something would be
/// invisible and everywhere.
pub fn install_shim(dir: &Path, force: bool) -> std::io::Result<Installed> {
    let target = std::env::current_exe()?;
    let link = dir.join("jq");

    if link.exists() || link.symlink_metadata().is_ok() {
        let existing = std::fs::read_link(&link).ok();
        let is_ours = existing.as_deref() == Some(target.as_path());
        if is_ours {
            return Ok(Installed::Created(link));
        }
        if !force {
            return Ok(Installed::Refused(format!(
                "{} already exists. Re-run with --force to replace it.",
                link.display()
            )));
        }
        std::fs::remove_file(&link)?;
    }

    std::os::unix::fs::symlink(&target, &link)?;
    Ok(Installed::Created(link))
}

/// Pull Pathfinder's own verbs out of argv before the jq parser sees them.
///
/// Returns the verb and the remaining arguments.
#[derive(Debug, PartialEq, Eq)]
pub enum Verb {
    Explain,
    InstallShim { dir: PathBuf, force: bool },
}

/// Extract a Pathfinder verb from `argv`, leaving the rest for the jq parser.
pub fn take_verb(argv: &mut Vec<OsString>) -> Option<Verb> {
    let pos = argv
        .iter()
        .position(|a| a == "--explain" || a == "--install-shim")?;
    let verb = argv.remove(pos);
    if verb == "--explain" {
        return Some(Verb::Explain);
    }
    let dir = if pos < argv.len() && !argv[pos].to_string_lossy().starts_with('-') {
        PathBuf::from(argv.remove(pos))
    } else {
        PathBuf::from(".")
    };
    let force = argv.iter().any(|a| a == "--force");
    argv.retain(|a| a != "--force");
    Some(Verb::InstallShim { dir, force })
}

#[cfg(test)]
mod tests {
    use super::{Verb, take_verb};
    use std::ffi::OsString;
    use std::path::PathBuf;

    fn argv(parts: &[&str]) -> Vec<OsString> {
        parts.iter().map(OsString::from).collect()
    }

    #[test]
    fn explain_is_lifted_out_of_the_jq_argv() {
        let mut a = argv(["--explain", "-c", ".a"].as_ref());
        assert_eq!(take_verb(&mut a), Some(Verb::Explain));
        assert_eq!(a, argv(["-c", ".a"].as_ref()));
    }

    #[test]
    fn install_shim_takes_an_optional_directory() {
        let mut a = argv(["--install-shim", "/tmp/bin"].as_ref());
        assert_eq!(
            take_verb(&mut a),
            Some(Verb::InstallShim {
                dir: PathBuf::from("/tmp/bin"),
                force: false
            })
        );
        assert_eq!(a, Vec::<OsString>::new());

        let mut a = argv(["--install-shim"].as_ref());
        assert_eq!(
            take_verb(&mut a),
            Some(Verb::InstallShim {
                dir: PathBuf::from("."),
                force: false
            })
        );
    }

    #[test]
    fn install_shim_reads_force() {
        let mut a = argv(["--install-shim", "/tmp/bin", "--force"].as_ref());
        assert_eq!(
            take_verb(&mut a),
            Some(Verb::InstallShim {
                dir: PathBuf::from("/tmp/bin"),
                force: true
            })
        );
    }

    #[test]
    fn an_ordinary_jq_argv_has_no_verb() {
        let mut a = argv(["-r", ".a", "in.json"].as_ref());
        assert_eq!(take_verb(&mut a), None);
        assert_eq!(a, argv(["-r", ".a", "in.json"].as_ref()));
    }
}
