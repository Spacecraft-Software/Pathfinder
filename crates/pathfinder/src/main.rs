// SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Pathfinder — a jq 1.8.1-compatible `jq` on a fork of jaq's engine.
//!
//! Installed as `pathfinder` with a `jq` symlink beside it. Under either name it
//! accepts jq's command line, rewrites the filter where the engine would run it
//! differently from jq, and evaluates it on the engine crates linked into this
//! binary, in process (`engine`).

mod compat;
mod describe;
mod diag;
mod engine;
mod front;
mod inputs;
mod jqargs;
mod json;
mod mode;
mod native;
mod post;
mod prelude;
mod program;
mod scan;
mod syntax;
mod translate;

use std::env;
use std::ffi::OsString;
use std::io::{self, IsTerminal as _};

/// The engine allocates per value; mimalloc halves the cycles of an
/// allocation-heavy filter against the system allocator (`Cargo.toml`).
#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// jq's own `--help` output, reproduced so the shim answers exactly as jq does.
const JQ_HELP: &str = include_str!("embed/jq-help.txt");

#[expect(
    clippy::similar_names,
    reason = "`args` (parsed) and `argv` (raw) are the domain's own names"
)]
fn main() {
    let mut argv: Vec<OsString> = env::args_os().collect();
    let invoked = compat::detect(&argv);
    let prog = invoked.prog();

    // Pathfinder's own verbs are lifted out first, and only under its own name:
    // through the `jq` symlink they stay unknown options, exactly as with jq.
    // The output mode is likewise decided only on the native side, so nothing
    // in the environment can reach a `jq` call's output.
    let native = invoked.allows_own_verbs();
    let lifted = if native {
        native::take_verb(&mut argv)
    } else {
        native::Lifted::default()
    };
    run_standalone_verb(&lifted);

    // A JSON-mode report (`--explain`, `--version`) fails with the Standard's
    // structured error instead of jq's text. `None` under the name `jq`, so
    // every failure below keeps jq's bytes there.
    let report = if native { lifted.json_report() } else { None };

    let mut args = match jqargs::parse(&argv[1.min(argv.len())..]) {
        Ok(a) => a,
        Err(e) => {
            if let Some(r) = &report {
                r.fail(&native::Failure::invalid_argument(&e.message));
            }
            diag::die(prog, &e.line(prog))
        }
    };

    if args.flags.help {
        print!("{JQ_HELP}");
        if native {
            eprint!("{}", native::help_trailer());
        }
        return;
    }
    if args.flags.version {
        if native {
            check_engine(prog, report.as_ref());
            native::version(mode::detect(lifted.json));
        } else {
            println!("{}", native::JQ_BASELINE_VERSION);
        }
        return;
    }
    if args.flags.build_configuration {
        println!(
            "pathfinder-{} (jq-compatible, on a fork of jaq's engine)",
            env!("CARGO_PKG_VERSION")
        );
        return;
    }
    // jq runs `.` when no filter is given and either end is not a terminal, so
    // `curl … | jq` pretty-prints. `-f` with no file is still a usage error.
    // Measured on jq 1.8.1 under a pseudo-terminal for each combination.
    if args.program.is_none()
        && !args.flags.from_file
        && !(io::stdin().is_terminal() && io::stdout().is_terminal())
    {
        args.program = Some(OsString::from("."));
    }
    if args.program.is_none() {
        if let Some(r) = &report {
            r.fail(&native::Failure::missing_filter());
        }
        // jq with no filter prints usage and exits 2.
        eprint!("{JQ_HELP}");
        std::process::exit(diag::EXIT_USAGE);
    }
    if args.flags.debug_dump_disasm || args.flags.debug_trace {
        diag::warn(
            prog,
            "jq's debug tracing has no equivalent in Pathfinder; the flag is ignored",
        );
    }

    check_engine(prog, report.as_ref());

    let plan = match translate::plan(&args, &read_filter_file) {
        Ok(p) => p,
        Err(translate::Refusal::Unsupported(u)) => {
            if let Some(r) = &report {
                r.fail(&native::Failure::unsupported(&u));
            }
            diag::unsupported(prog, &u.feature, &u.detail)
        }
        Err(translate::Refusal::Compile { error, source }) => {
            if let Some(r) = &report {
                r.fail(&native::Failure::compile(&error, &source));
            }
            eprint!("{}", error.render(prog, &source));
            std::process::exit(diag::EXIT_COMPILE);
        }
    };

    if matches!(lifted.verb, Some(native::Verb::Explain)) {
        native::explain(&plan, mode::detect(lifted.json));
        return;
    }

    std::process::exit(engine::run(&plan));
}

/// Run a native verb that takes no jq call, and end the process; any other
/// verb returns.
fn run_standalone_verb(lifted: &native::Lifted) {
    let mode = || mode::detect(lifted.json);
    match &lifted.verb {
        Some(native::Verb::InstallShim { dir, force }) => {
            std::process::exit(native::run_install_shim(dir, *force, mode()));
        }
        Some(native::Verb::Describe) => {
            describe::describe(mode());
            std::process::exit(0);
        }
        Some(native::Verb::Schema { command, format }) => {
            std::process::exit(native::run_schema(
                command.as_ref(),
                format.as_ref(),
                mode(),
            ));
        }
        Some(native::Verb::Explain) | None => {}
    }
}

/// End the process if `PATHFINDER_ENGINE` asks for an engine other than
/// the one there is. That is the caller's environment, not jq's command
/// line, so it is reported in Pathfinder's name, with jq's usage status.
fn check_engine(prog: &str, report: Option<&native::JsonReport>) {
    if let Err(e) = engine::check() {
        if let Some(r) = report {
            r.fail(&native::Failure::invalid_argument(&e));
        }
        eprintln!("{prog}: error: {e}");
        std::process::exit(diag::EXIT_USAGE);
    }
}

/// Read a `-f` filter file.
fn read_filter_file(path: &OsString) -> io::Result<String> {
    std::fs::read_to_string(path)
}
