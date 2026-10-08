// SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Pathfinder — a `jq`-compatible shim over `jaq`.
//!
//! Installed as `pathfinder` with a `jq` symlink beside it. Under either name it
//! accepts jq's command line, translates it into jaq's, rewrites the filter when
//! a polyfill is needed, and hands off to jaq.
//!
//! The hand-off has two shapes, and which one runs is the design's central
//! trade-off:
//!
//! - **`exec`** — the process image is replaced by jaq's. Nothing is copied, no
//!   second process exists, and the cost over running jaq directly is one
//!   `execve`. This is the path for every invocation that needs no output
//!   transform, which is nearly all of them.
//! - **spawn and pipe** — only for `-a` and `--seq`, where jq's behaviour cannot
//!   be expressed in jaq's argv and the bytes have to be reshaped on the way
//!   out.
//!
//! The cost of `exec` is that Pathfinder cannot observe jaq's exit code or
//! stderr, so it cannot retouch either. That is a deliberate trade: the
//! divergences it could fix that way are documented in `doc/DIVERGENCES.md`,
//! and none of them is worth making every invocation pay for a second process.

mod compat;
mod diag;
mod jqargs;
mod native;
mod post;
mod prelude;
mod program;
mod scan;
mod translate;

use std::env;
use std::ffi::OsString;
use std::io;
use std::os::unix::process::{CommandExt as _, ExitStatusExt as _};
use std::process::{Command, Stdio};

/// jq's own `--help` output, reproduced so the shim answers exactly as jq does.
const JQ_HELP: &str = include_str!("embed/jq-help.txt");

/// The jq release Pathfinder reproduces.
///
/// Reported by `--version` because scripts grep for `jq-1.` to gate features;
/// answering with a Pathfinder version string would fail those checks.
const JQ_BASELINE_VERSION: &str = "jq-1.8.1";

/// Environment variable naming the jaq binary, for testing and for packaging
/// that wants to pin a store path rather than trust `PATH`.
const JAQ_ENV: &str = "PATHFINDER_JAQ";

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
    let verb = if invoked.allows_own_verbs() {
        native::take_verb(&mut argv)
    } else {
        None
    };
    if let Some(native::Verb::InstallShim { dir, force }) = &verb {
        match native::install_shim(dir, *force) {
            Ok(native::Installed::Created(p)) => {
                println!("installed {} -> this binary", p.display());
                println!(
                    "add {} to PATH ahead of any real jq to use it.",
                    dir.display()
                );
            }
            Ok(native::Installed::Refused(why)) => {
                eprintln!("{prog}: {why}");
                std::process::exit(1);
            }
            Err(e) => {
                eprintln!("{prog}: could not install the shim: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    let args = match jqargs::parse(&argv[1.min(argv.len())..]) {
        Ok(a) => a,
        Err(e) => diag::die(prog, &e.message),
    };

    if args.flags.help {
        print!("{JQ_HELP}");
        if invoked.allows_own_verbs() {
            eprintln!(
                "\n(pathfinder {}: a jq-compatible shim over jaq)",
                env!("CARGO_PKG_VERSION")
            );
        }
        return;
    }
    if args.flags.version {
        println!("{JQ_BASELINE_VERSION}");
        return;
    }
    if args.flags.build_configuration {
        println!(
            "pathfinder-{} (jq-compatible shim over jaq)",
            env!("CARGO_PKG_VERSION")
        );
        return;
    }
    if args.program.is_none() {
        // jq with no filter prints usage and exits 2.
        eprint!("{JQ_HELP}");
        std::process::exit(diag::EXIT_USAGE);
    }
    if args.flags.debug_dump_disasm || args.flags.debug_trace {
        diag::warn(
            prog,
            "jq's debug tracing has no jaq equivalent; the flag is ignored",
        );
    }

    let plan = match translate::plan(&args, &read_filter_file) {
        Ok(p) => p,
        Err(u) => diag::unsupported(prog, &u.feature, &u.detail),
    };

    if matches!(verb, Some(native::Verb::Explain)) {
        native::explain(&args, &plan);
        return;
    }

    // `exec` never returns: it replaces this process or exits.
    if plan.post.is_identity() && !plan.strip_input_rs && plan.concat_inputs.is_none() {
        exec(prog, &plan.argv);
    }
    std::process::exit(pipe(prog, &plan));
}

/// Write jaq's input: the concatenated files, or stdin, with RS stripped if
/// `--seq` asked for it.
///
/// Concatenating restores jq's "several files are one stream" semantics, which
/// jaq does not share. `-` keeps its jq meaning of "stdin here".
fn feed(sink: &mut impl io::Write, files: Option<&[OsString]>, strip: bool) -> io::Result<()> {
    let Some(files) = files else {
        let stdin = io::stdin();
        return if strip {
            post::strip_rs(stdin.lock(), sink)
        } else {
            io::copy(&mut stdin.lock(), sink).map(|_| ())
        };
    };
    for path in files {
        if path == "-" {
            let stdin = io::stdin();
            if strip {
                post::strip_rs(stdin.lock(), sink)?;
            } else {
                io::copy(&mut stdin.lock(), sink)?;
            }
            continue;
        }
        let mut file = std::fs::File::open(path)?;
        if strip {
            post::strip_rs(&mut file, sink)?;
        } else {
            io::copy(&mut file, sink)?;
        }
        // jq inserts nothing between files, but a file with no trailing newline
        // would otherwise glue its last token onto the next file's first.
        sink.write_all(b"\n")?;
    }
    Ok(())
}

/// Read a `-f` filter file.
fn read_filter_file(path: &OsString) -> io::Result<String> {
    std::fs::read_to_string(path)
}

/// The jaq a packager pinned at build time, if any.
///
/// Set by `packaging/default.nix` to a store path. It is compiled in rather
/// than supplied by a wrapper script because a wrapper would put a shell
/// process in front of every `jq` call, losing the single-`execve` fast path
/// this binary exists to keep.
const PINNED_JAQ: Option<&str> = option_env!("PATHFINDER_DEFAULT_JAQ");

/// Locate the jaq binary: the runtime override, then the build-time pin, then
/// whatever `jaq` resolves to on `PATH`.
fn jaq_binary() -> OsString {
    env::var_os(JAQ_ENV)
        .or_else(|| PINNED_JAQ.map(OsString::from))
        .unwrap_or_else(|| OsString::from("jaq"))
}

/// Replace this process with jaq. Only returns on failure.
fn exec(prog: &str, argv: &[OsString]) -> ! {
    let jaq = jaq_binary();
    let err = Command::new(&jaq).args(argv).exec();
    // `exec` returns only when the replacement failed.
    eprintln!("{prog}: could not execute {}: {err}", jaq.to_string_lossy());
    eprintln!("{prog}: pathfinder is a shim over jaq; set {JAQ_ENV} if jaq is not on PATH.");
    std::process::exit(127);
}

/// Run jaq as a child, reshaping its output, and return the code to exit with.
///
/// stderr is inherited rather than captured, so jaq's diagnostics, `debug`
/// output and colour decisions reach the terminal untouched.
fn pipe(prog: &str, plan: &translate::Plan) -> i32 {
    let jaq = jaq_binary();
    let mut command = Command::new(&jaq);
    command
        .args(&plan.argv)
        .stderr(Stdio::inherit())
        .stdout(Stdio::piped());
    let feed_stdin = plan.strip_input_rs || plan.concat_inputs.is_some();
    if feed_stdin {
        command.stdin(Stdio::piped());
    } else {
        command.stdin(Stdio::inherit());
    }

    let mut child = match command.spawn() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{prog}: could not execute {}: {e}", jaq.to_string_lossy());
            return 127;
        }
    };

    // Feed jaq's stdin ourselves when the input needs reshaping. A thread keeps
    // it streaming rather than reading everything before jaq starts.
    let strip = plan.strip_input_rs;
    let files = plan.concat_inputs.clone();
    let feeder = child.stdin.take().map(|mut sink| {
        std::thread::spawn(move || {
            let _ = feed(&mut sink, files.as_deref(), strip);
        })
    });

    let mut code = 0;
    if let Some(out) = child.stdout.take() {
        let stdout = io::stdout();
        let mut lock = stdout.lock();
        if let Err(e) = post::transform(out, &mut lock, plan.post) {
            // A closed downstream reader is normal for a filter in a pipeline.
            if e.kind() == io::ErrorKind::BrokenPipe {
                return 141;
            }
            eprintln!("{prog}: error writing output: {e}");
            code = 2;
        }
    }
    if let Some(handle) = feeder {
        let _ = handle.join();
    }

    match child.wait() {
        Ok(status) => status.code().unwrap_or_else(|| {
            // Killed by a signal: report it the way a shell would.
            status.signal().map_or(1, |s| 128 + s)
        }),
        Err(e) => {
            eprintln!("{prog}: could not wait for {}: {e}", jaq.to_string_lossy());
            1
        }
    }
    .max(code)
}
