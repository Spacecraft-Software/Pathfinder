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
mod inputs;
mod jqargs;
mod json;
mod mode;
mod native;
mod post;
mod prelude;
mod program;
mod regex;
mod scan;
mod syntax;
mod translate;

use std::env;
use std::ffi::OsString;
use std::io::{self, IsTerminal as _};
use std::os::unix::process::{CommandExt as _, ExitStatusExt as _};
use std::process::{Command, Stdio};

/// jq's own `--help` output, reproduced so the shim answers exactly as jq does.
const JQ_HELP: &str = include_str!("embed/jq-help.txt");

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
    // The output mode is likewise decided only on the native side, so nothing
    // in the environment can reach a `jq` call's output.
    let native = invoked.allows_own_verbs();
    let lifted = if native {
        native::take_verb(&mut argv)
    } else {
        native::Lifted::default()
    };
    if let Some(native::Verb::InstallShim { dir, force }) = &lifted.verb {
        let mode = mode::detect(lifted.json);
        std::process::exit(native::run_install_shim(dir, *force, mode));
    }

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
            let (path, source) = jaq_binary();
            let engine = native::Engine {
                path: &path,
                source: source.label(),
            };
            native::version(&engine, mode::detect(lifted.json));
        } else {
            println!("{}", native::JQ_BASELINE_VERSION);
        }
        return;
    }
    if args.flags.build_configuration {
        println!(
            "pathfinder-{} (jq-compatible shim over jaq)",
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
            "jq's debug tracing has no jaq equivalent; the flag is ignored",
        );
    }

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

    // `exec` never returns: it replaces this process or exits.
    if plan.post.is_identity() && !plan.strip_input_rs && plan.concat_inputs.is_none() {
        exec(prog, &plan.argv);
    }
    std::process::exit(pipe(prog, &plan));
}

/// Write jaq's stdin, with RS stripped if `--seq` asked for it.
fn feed(sink: &mut impl io::Write, strip: bool) -> io::Result<()> {
    let mut stdin = io::stdin().lock();
    if strip {
        post::strip_rs(&mut stdin, sink)
    } else {
        io::copy(&mut stdin, sink).map(|_| ())
    }
}

/// Whether `jaq` is Pathfinder's patched jaq, which reports an unreadable
/// input file itself. Asked only when there is one, so it costs nothing on
/// any other call.
fn engine_is_patched(jaq: &OsString) -> bool {
    Command::new(jaq)
        .arg("--version")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains("+pathfinder"))
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

/// Which step of the lookup in [`jaq_binary`] supplied the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JaqSource {
    /// The [`JAQ_ENV`] runtime override.
    Env,
    /// The build-time pin, [`PINNED_JAQ`].
    Pinned,
    /// Whatever `jaq` resolves to on `PATH`.
    Path,
}

impl JaqSource {
    /// The name `pathfinder --version` reports.
    const fn label(self) -> &'static str {
        match self {
            Self::Env => JAQ_ENV,
            Self::Pinned => "pinned",
            Self::Path => "PATH",
        }
    }
}

/// Locate the jaq binary: the runtime override, then the build-time pin, then
/// whatever `jaq` resolves to on `PATH`.
fn jaq_binary() -> (OsString, JaqSource) {
    if let Some(jaq) = env::var_os(JAQ_ENV) {
        return (jaq, JaqSource::Env);
    }
    match PINNED_JAQ {
        Some(jaq) => (OsString::from(jaq), JaqSource::Pinned),
        None => (OsString::from("jaq"), JaqSource::Path),
    }
}

/// Replace this process with jaq. Only returns on failure.
fn exec(prog: &str, argv: &[OsString]) -> ! {
    let (jaq, _) = jaq_binary();
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
    let (jaq, _) = jaq_binary();
    let mut command = Command::new(&jaq);
    command
        .args(&plan.argv)
        .stderr(Stdio::inherit())
        .stdout(Stdio::piped());
    // Several files are opened before jaq starts, so that they can be
    // described to it (see `inputs`).
    let strip = plan.strip_input_rs;
    let inputs = plan
        .concat_inputs
        .as_deref()
        .map(|files| inputs::open(files, strip));
    let mut unreported = false;
    if let Some(inputs) = &inputs {
        command.env(inputs::INPUTS_ENV, inputs::describe(inputs));
        // A stock jaq does not know of unreadable files: say so here, if not
        // where jq would, and exit as jq would.
        if inputs.iter().any(|i| i.failure().is_some()) && !engine_is_patched(&jaq) {
            unreported = true;
            for m in inputs.iter().filter_map(inputs::Input::failure) {
                eprintln!("jq: error: {m}");
            }
        }
    }
    let feed_stdin = plan.strip_input_rs || inputs.is_some();
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
    let feeder = child.stdin.take().map(|mut sink| {
        std::thread::spawn(move || {
            let _ = match inputs {
                Some(inputs) => inputs::feed(&mut sink, inputs, strip),
                None => feed(&mut sink, strip),
            };
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

    let code = match child.wait() {
        Ok(status) => status.code().unwrap_or_else(|| {
            // Killed by a signal: report it the way a shell would.
            status.signal().map_or(1, |s| 128 + s)
        }),
        Err(e) => {
            eprintln!("{prog}: could not wait for {}: {e}", jaq.to_string_lossy());
            1
        }
    }
    .max(code);
    // jq exits with 2 when an input file could not be read, unless killed.
    if unreported && code < 128 { 2 } else { code }
}
