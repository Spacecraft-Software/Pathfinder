// SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Pathfinder's own verbs, reachable only under its own name.
//!
//! These are deliberately unavailable through the `jq` symlink. A script that
//! passes `--explain` to what it believes is jq must get jq's `Unknown option`
//! error, not a Pathfinder feature — the drop-in surface has to *be* jq's
//! surface, or it is not a drop-in.
//!
//! Each verb reports in one of two modes ([`Mode`]): the human text, or one
//! compact JSON document in the Spacecraft CLI Standard's envelope,
//! `{"metadata":{…},"data":{…}}`. Every failure of a report in JSON mode —
//! a usage error, a missing filter, a compile error, an unsupported feature,
//! an install conflict — goes to stderr as one `{"error":{…}}` line. Under the
//! name `jq` none of this is reachable.

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::describe;
use crate::json::Object;
use crate::mode::{self, Mode};
use crate::syntax::check::CompileError;
use crate::translate::{Plan, Unsupported};
use crate::{diag, jqargs};

/// The jq release Pathfinder reproduces.
///
/// Reported by `jq --version` because scripts grep for `jq-1.` to gate
/// features; answering with a Pathfinder version string would fail those
/// checks.
pub const JQ_BASELINE_VERSION: &str = "jq-1.8.1";

/// The maintainer, as the Standard's attribution clause (§15) names him.
pub const MAINTAINER: &str = "Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>";

/// The project's registered subdomain (Standard §15.1); also the docs URL in
/// structured errors.
pub const WEBSITE: &str = "https://Pathfinder.SpacecraftSoftware.org/";

/// The `--help` trailer printed on stderr under the native name only, after
/// jq's own help text on stdout.
///
/// It documents the options that exist only under this name; jq's help above
/// it cannot, and `jq --help` must not.
pub fn help_trailer() -> String {
    format!(
        "\n(pathfinder {}: a jq 1.8.1-compatible jq on a fork of jaq's engine)\n\
         \n\
         Pathfinder's own options (not available as `jq`):\n      \
         --explain             show the engine and program a call would run, run nothing;\n      \
         --version             Pathfinder's version and the engine it runs;\n      \
         --install-shim [DIR]  link DIR/jq (default .) to this binary;\n      \
         --force               with --install-shim, replace an existing jq;\n      \
         --describe            list these commands, the environment and conventions;\n      \
         --schema [COMMAND]    every command's JSON Schema, or COMMAND's;\n      \
         --format FORMAT       with --schema: json, anthropic, openai, gemini or mcp;\n      \
         --json                report as one JSON document; the default under\n                            \
         AI_AGENT, AGENT, a truthy CI, or a non-terminal stdout;\n\
         \n\
         Examples:\n    \
         pathfinder --explain -c '.a.b = 1'\n    \
         pathfinder --explain --json -c '.a.b = 1'\n    \
         pathfinder --version --json\n    \
         pathfinder --describe --json\n    \
         pathfinder --schema --format anthropic\n    \
         pathfinder --install-shim ~/.local/bin\n\
         \n\
         Maintained by {MAINTAINER}\n{WEBSITE}\n",
        env!("CARGO_PKG_VERSION")
    )
}

/// Show what Pathfinder would run, without running it.
///
/// This is the debugging entry point: when a filter behaves unexpectedly, the
/// first question is always "what did jaq actually receive", and guessing at it
/// from the source is how subtle rewriting bugs survive.
pub fn explain(plan: &Plan, engine: crate::engine::Engine, mode: Mode) {
    match mode {
        Mode::Json => println!(
            "{}",
            explain_json(plan, engine, &Meta::now("pathfinder --explain"))
        ),
        Mode::Human => explain_human(plan, engine),
    }
}

/// How the plan would run: in process, or by `exec` or a pipe to jaq.
fn execution(plan: &Plan, engine: crate::engine::Engine) -> &'static str {
    match engine {
        crate::engine::Engine::Native => "in-process",
        crate::engine::Engine::Jaq if needs_pipe(plan) => "pipe",
        crate::engine::Engine::Jaq => "exec",
    }
}

/// The plan as the JSON envelope's `data`.
fn explain_json(plan: &Plan, engine: crate::engine::Engine, meta: &Meta) -> String {
    let argv: Vec<String> = plan
        .argv
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let program = match &plan.assembly {
        Some(a) => Object::new()
            .bool("rewritten", true)
            .nonempty_strs("injected", &a.injected)
            .nonempty_strs("shadowed", &a.shadowed)
            .bool("syntax_rewritten", a.syntax_rewritten)
            .str("text", &a.text),
        None => Object::new().bool("rewritten", false),
    };
    let mut stdin = Object::new().bool("strip_rs", plan.strip_input_rs);
    if let Some(files) = &plan.concat_inputs {
        stdin = stdin.int(
            "concat_files",
            i64::try_from(files.len()).unwrap_or(i64::MAX),
        );
    }
    let data = Object::new()
        .str("engine", engine.name())
        .strs("jaq_argv", &argv)
        .obj("program", program)
        .str("execution", execution(plan, engine))
        .obj(
            "stdout",
            Object::new()
                .bool("ascii", plan.post.ascii)
                .bool("seq", plan.post.seq),
        )
        .obj("stdin", stdin);
    envelope(meta, data)
}

/// The plan as today's human-readable text.
fn explain_human(plan: &Plan, engine: crate::engine::Engine) {
    println!("engine: {}", engine.name());
    println!("\njaq argv (the jaq engine's command line):");
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
        match execution(plan, engine) {
            "in-process" => "in process (no other process)",
            "pipe" => "spawn + pipe",
            _ => "exec (no extra process)",
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

    if let Some(a) = &plan.assembly
        && a.syntax_rewritten
    {
        println!(
            "\nnote: Pathfinder rewrote parts of this filter that jaq would run \
             differently from jq (assignment, deletion, indices, slices, @base64d and \
             others). Set PATHFINDER_NO_REWRITE=1 to compare with jaq's own behaviour."
        );
    }
}

/// Whether the plan gives up the `exec` fast path.
fn needs_pipe(plan: &Plan) -> bool {
    !plan.post.is_identity() || plan.strip_input_rs || plan.concat_inputs.is_some()
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

/// Run `--install-shim` and report in `mode`; returns the exit code.
///
/// Exit codes follow the CLI Standard: 5 for a conflict (a `jq` that is not
/// ours already exists and `--force` was not given), 4 when the directory is
/// not writable, 1 for any other failure.
pub fn run_install_shim(dir: &Path, force: bool, mode: Mode) -> i32 {
    const COMMAND: &str = "pathfinder --install-shim";
    let failure = match install_shim(dir, force) {
        Ok(Installed::Created(link)) => {
            match mode {
                Mode::Human => {
                    println!("installed {} -> this binary", link.display());
                    println!(
                        "add {} to PATH ahead of any real jq to use it.",
                        dir.display()
                    );
                }
                Mode::Json => {
                    // The link target is this binary; `install_shim` already
                    // resolved it once, so failing here is not expected.
                    let target = std::env::current_exe()
                        .map(|t| t.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let data = Object::new()
                        .str("installed", &link.to_string_lossy())
                        .str("target", &target);
                    println!("{}", envelope(&Meta::now(COMMAND), data));
                }
            }
            return 0;
        }
        Ok(Installed::Refused(why)) => Failure {
            code: "CONFLICT",
            exit: EXIT_CONFLICT,
            human: why.clone(),
            message: why,
            hint: Some(format!("{COMMAND} {} --force", shell_quote(dir))),
        },
        Err(e) => {
            // Every hint changes something: re-running the failed command as
            // it was would fail the same way.
            let elsewhere = format!("{COMMAND} {FALLBACK_DIR}");
            let (code, exit, hint) = match e.kind() {
                io::ErrorKind::PermissionDenied => {
                    ("PERMISSION_DENIED", EXIT_PERMISSION, elsewhere)
                }
                io::ErrorKind::NotFound => (
                    "GENERAL",
                    EXIT_GENERAL,
                    format!("mkdir -p {0} && {COMMAND} {0}", shell_quote(dir)),
                ),
                // `DIR` is a file: its own directory is the likely intent.
                io::ErrorKind::NotADirectory => (
                    "GENERAL",
                    EXIT_GENERAL,
                    dir.parent()
                        .filter(|p| !p.as_os_str().is_empty())
                        .map_or(elsewhere, |p| format!("{COMMAND} {}", shell_quote(p))),
                ),
                // Anything else (`DIR/jq` is a directory, a read-only file
                // system, …) is not fixed by retrying here.
                _ => ("GENERAL", EXIT_GENERAL, elsewhere),
            };
            Failure {
                code,
                exit,
                human: format!("could not install the shim: {e}"),
                message: format!("could not install the shim in {}: {e}", dir.display()),
                hint: Some(hint),
            }
        }
    };
    match mode {
        Mode::Human => eprintln!("pathfinder: {}", failure.human),
        Mode::Json => eprintln!("{}", failure.render(COMMAND, &utc_now())),
    }
    failure.exit
}

/// `pathfinder --schema`: the document, or a usage error naming what is
/// accepted. Returns the exit status.
pub fn run_schema(command: Option<&OsString>, format: Option<&OsString>, mode: Mode) -> i32 {
    const COMMAND: &str = "pathfinder --schema";
    let text = |s: Option<&OsString>| s.map(|s| s.to_string_lossy().into_owned());
    let (command, format) = (text(command), text(format));
    let failure = match format
        .as_deref()
        .map_or(Some(describe::Format::Json), describe::Format::parse)
    {
        None => Failure::reported(
            "INVALID_ARGUMENT",
            diag::EXIT_USAGE,
            format!(
                "--format {} is not one of json, anthropic, openai, gemini, mcp",
                format.unwrap_or_default()
            ),
            Some("pathfinder --schema --format anthropic"),
        ),
        Some(f) => match describe::schema(command.as_deref(), f) {
            Some(doc) => {
                println!("{doc}");
                return 0;
            }
            None => Failure::reported(
                "INVALID_ARGUMENT",
                diag::EXIT_USAGE,
                format!(
                    "{} is not a command; one of {}",
                    command.unwrap_or_default(),
                    describe::tool_names()
                ),
                Some("pathfinder --schema pathfinder_explain"),
            ),
        },
    };
    match mode {
        Mode::Human => eprintln!("pathfinder: {}", failure.human),
        Mode::Json => eprintln!("{}", failure.render(COMMAND, &utc_now())),
    }
    failure.exit
}

/// Exit code for a failure with no more specific class.
const EXIT_GENERAL: i32 = 1;

/// Exit code for a permission failure (CLI Standard exit-code table).
const EXIT_PERMISSION: i32 = 4;

/// Exit code for a conflict with existing state (CLI Standard exit-code table).
const EXIT_CONFLICT: i32 = 5;

/// Where an install hint points when the requested directory cannot work: the
/// per-user binary directory of the XDG base-directory layout, which needs no
/// privileges. Left unquoted in hints so the shell expands `~`.
const FALLBACK_DIR: &str = "~/.local/bin";

/// A native verb's failure, in both of its renderings.
#[derive(Debug)]
pub struct Failure {
    /// The Standard's error code, e.g. `CONFLICT`.
    code: &'static str,
    exit: i32,
    /// The text human mode prints after `pathfinder: `, unchanged from before
    /// the JSON mode existed.
    human: String,
    message: String,
    /// A runnable command that addresses the failure. Omitted where none is
    /// known, as the Standard asks, rather than guessed.
    hint: Option<String>,
}

impl Failure {
    /// A command line jq's grammar rejects. Exit 2, jq's usage code.
    pub fn invalid_argument(message: &str) -> Self {
        Self::reported(
            "INVALID_ARGUMENT",
            diag::EXIT_USAGE,
            message.to_owned(),
            Some("pathfinder --help"),
        )
    }

    /// `--explain` with no filter to explain. Exit 2, as jq exits without one.
    pub fn missing_filter() -> Self {
        Self::reported(
            "MISSING_ARGUMENT",
            diag::EXIT_USAGE,
            "no jq filter was given to explain".to_owned(),
            Some("pathfinder --explain --json ."),
        )
    }

    /// A jq feature jaq cannot provide. Exit 2, as in human mode.
    pub fn unsupported(u: &Unsupported) -> Self {
        Self::reported(
            "UNSUPPORTED",
            diag::EXIT_USAGE,
            format!("{} is not supported by jaq: {}", u.feature, u.detail),
            None,
        )
    }

    /// A program jq refuses to compile. Exit 3, jq's compile code, although
    /// the Standard's table gives 3 to `NOT_FOUND`: the exit code is the one a
    /// `jq` call would see, so it cannot differ between the two names.
    pub fn compile(error: &CompileError, source: &str) -> Self {
        // The first line of jq's rendering, without its `prog: error: ` prefix
        // and the colon that introduces the caret block.
        let rendered = error.render("", source);
        let first = rendered.lines().next().unwrap_or_default();
        let message = first
            .strip_prefix(": error: ")
            .unwrap_or(first)
            .trim_end_matches(':');
        Self::reported(
            "COMPILE_ERROR",
            diag::EXIT_COMPILE,
            message.to_owned(),
            None,
        )
    }

    /// A failure reported only in JSON mode: human mode keeps jq's own text.
    fn reported(code: &'static str, exit: i32, message: String, hint: Option<&str>) -> Self {
        Self {
            code,
            exit,
            human: message.clone(),
            message,
            hint: hint.map(str::to_owned),
        }
    }

    /// The structured error for stderr.
    fn render(&self, command: &str, timestamp: &str) -> String {
        let error = Object::new()
            .str("code", self.code)
            .int("exit_code", i64::from(self.exit))
            .str("message", &self.message)
            .opt_str("hint", self.hint.as_deref())
            .str("timestamp", timestamp)
            .str("command", command)
            .str("docs_url", WEBSITE);
        Object::new().obj("error", error).finish()
    }
}

/// A report that `--explain` or `--version` asked for, in JSON mode.
///
/// Holding one is what routes a failure to the structured error. It exists only
/// under the native name, only when a report was asked for, and only in JSON
/// mode; everywhere else failures keep jq's text, byte for byte.
#[derive(Debug)]
pub struct JsonReport {
    command: &'static str,
}

impl JsonReport {
    /// Fail the report: the structured error on stderr, then exit.
    pub fn fail(&self, failure: &Failure) -> ! {
        eprintln!("{}", failure.render(self.command, &utc_now()));
        std::process::exit(failure.exit);
    }
}

/// Quote `path` for a POSIX shell when it holds anything but safe characters,
/// so a hint stays runnable for a directory with spaces in its name.
fn shell_quote(path: &Path) -> String {
    let text = path.to_string_lossy();
    let safe = !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-+~,:@%=".contains(c));
    if safe && !text.starts_with('~') {
        text.into_owned()
    } else {
        format!("'{}'", text.replace('\'', "'\\''"))
    }
}

/// The engine a filter would run on.
#[derive(Debug)]
pub struct Engine<'a> {
    /// `native` or `jaq`.
    pub name: &'static str,
    /// What chose it: `PATHFINDER_ENGINE`, `built-in` or `default`.
    pub selected_by: &'static str,
    /// For the `jaq` engine, the binary as the lookup resolved it (`jaq` for
    /// a `PATH` lookup) and which step supplied it: `PATHFINDER_JAQ`,
    /// `pinned` or `PATH`.
    pub jaq: Option<(&'a OsStr, &'static str)>,
}

/// The path to report: a bare `jaq` is resolved against `PATH` for display
/// (without spawning it), so the reader sees which file would run.
fn display_path(path: &OsStr) -> String {
    let bare = Path::new(path).components().count() == 1;
    if bare && let Some(dirs) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&dirs) {
            let candidate = dir.join(path);
            if candidate.is_file() {
                return candidate.to_string_lossy().into_owned();
            }
        }
    }
    path.to_string_lossy().into_owned()
}

/// `pathfinder --version`: Pathfinder's own version, the jq it reproduces, and
/// the engine it would hand off to.
pub fn version(engine: &Engine<'_>, mode: Mode) {
    match mode {
        Mode::Human => print!("{}", version_human(engine)),
        Mode::Json => println!(
            "{}",
            version_json(engine, &Meta::now("pathfinder --version"))
        ),
    }
}

fn version_human(engine: &Engine<'_>) -> String {
    let line = match engine.jaq {
        None => format!(
            "engine: {}, in process ({})",
            engine.name, engine.selected_by
        ),
        Some((path, source)) => format!(
            "engine: {} ({}), {} ({source})",
            engine.name,
            engine.selected_by,
            display_path(path)
        ),
    };
    format!(
        "pathfinder {} ({JQ_BASELINE_VERSION} compatible)\n\
         {line}\n\
         Maintained by {MAINTAINER}\n\
         {WEBSITE}\n",
        env!("CARGO_PKG_VERSION"),
    )
}

fn version_json(engine: &Engine<'_>, meta: &Meta) -> String {
    let mut about = Object::new()
        .str("name", engine.name)
        .str("selected_by", engine.selected_by);
    if let Some((path, source)) = engine.jaq {
        about = about.str("path", &display_path(path)).str("source", source);
    }
    let data = Object::new()
        .str("version", env!("CARGO_PKG_VERSION"))
        .str("jq_baseline", JQ_BASELINE_VERSION)
        .obj("engine", about);
    envelope(meta, data)
}

/// The envelope's `metadata` for one report.
#[derive(Debug)]
pub struct Meta {
    command: &'static str,
    timestamp: String,
    invoking_agent: Option<&'static str>,
}

impl Meta {
    /// Metadata for `command`, stamped now.
    pub fn now(command: &'static str) -> Self {
        Self {
            command,
            timestamp: utc_now(),
            invoking_agent: invoking_agent(|name| std::env::var_os(name)),
        }
    }
}

/// The agent harness running this process, when it announces itself.
///
/// Informational only: unlike `AI_AGENT`, these variables never change the
/// output mode, they only label the report.
fn invoking_agent(var: impl Fn(&str) -> Option<OsString>) -> Option<&'static str> {
    [
        ("CLAUDECODE", "claude-code"),
        ("CURSOR_AGENT", "cursor"),
        ("GEMINI_CLI", "gemini-cli"),
    ]
    .into_iter()
    .find(|(name, _)| var(name).is_some_and(|v| !v.is_empty()))
    .map(|(_, agent)| agent)
}

/// Wrap `data` in the Standard's `{"metadata":…,"data":…}` envelope.
pub fn envelope(meta: &Meta, data: Object) -> String {
    let metadata = Object::new()
        .str("tool", "pathfinder")
        .str("version", env!("CARGO_PKG_VERSION"))
        .str("command", meta.command)
        .str("timestamp", &meta.timestamp)
        .str("maintainer", MAINTAINER)
        .str("website", WEBSITE)
        .opt_str("invoking_agent", meta.invoking_agent);
    Object::new()
        .obj("metadata", metadata)
        .obj("data", data)
        .finish()
}

/// The current time as ISO 8601 UTC, `YYYY-MM-DDTHH:MM:SSZ` (Standard §14).
///
/// A clock before 1970 reports the epoch rather than failing a report over it.
fn utc_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    format_utc(secs)
}

/// Seconds since the Unix epoch as `YYYY-MM-DDTHH:MM:SSZ`.
fn format_utc(secs: u64) -> String {
    let (days, rem) = (secs / SECS_PER_DAY, secs % SECS_PER_DAY);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

const SECS_PER_DAY: u64 = 86_400;

/// Days since 1970-01-01 to a proleptic Gregorian `(year, month, day)`.
///
/// Howard Hinnant's `civil_from_days`
/// (<https://howardhinnant.github.io/date_algorithms.html>), restricted to
/// non-negative day counts, which removes the era sign correction. It works in
/// 400-year eras of 146 097 days, counting years from March so the leap day
/// falls at the end of the year.
fn civil_from_days(days: u64) -> (u64, u64, u64) {
    // Shift the epoch from 1970-01-01 to 0000-03-01.
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z % 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11], March = 0
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let y = yoe + era * 400 + u64::from(m <= 2);
    (y, m, d)
}

/// Pathfinder's own verb, lifted out of argv.
#[derive(Debug, PartialEq, Eq)]
pub enum Verb {
    Explain,
    InstallShim {
        dir: PathBuf,
        force: bool,
    },
    Describe,
    /// `--schema [COMMAND] [--format FORMAT]`, as written: checked when run.
    Schema {
        command: Option<OsString>,
        format: Option<OsString>,
    },
}

/// What [`take_verb`] lifted out of argv.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Lifted {
    pub verb: Option<Verb>,
    /// `--json` was given to a native verb or to `--version`.
    pub json: bool,
    /// `--version` (or `-V`) is among the options, so the run reports.
    pub version: bool,
}

impl Lifted {
    /// The JSON report this run makes, when it makes one: `--explain` or
    /// `--version`, in JSON mode by the cascade. Reads the environment only
    /// when a report was asked for.
    pub fn json_report(&self) -> Option<JsonReport> {
        let command = match self.verb {
            Some(Verb::Explain) => "pathfinder --explain",
            Some(Verb::Describe) => "pathfinder --describe",
            Some(Verb::Schema { .. }) => "pathfinder --schema",
            None if self.version => "pathfinder --version",
            // `--install-shim` decides its own mode and failures.
            Some(Verb::InstallShim { .. }) | None => return None,
        };
        (mode::detect(self.json) == Mode::Json).then_some(JsonReport { command })
    }
}

/// Pull Pathfinder's own verbs, and `--json`, out of argv before the jq parser
/// sees them; the rest is left for the jq parser.
///
/// Only entries in option position count ([`jqargs::option_positions`]): an
/// option's value or a word after `--` is the user's data, so
/// `--arg x --json` keeps binding `$x` to `--json`. `--json` is lifted only
/// beside `--explain`, `--install-shim` or `--version` (`-V`). Anywhere else it
/// stays, so the jq parser rejects it as jq would — the flag belongs to
/// Pathfinder's surface, not to jq's.
pub fn take_verb(argv: &mut Vec<OsString>) -> Lifted {
    // `argv[0]` is the program name, never an option.
    let options: Vec<usize> = jqargs::option_positions(argv.get(1..).unwrap_or_default())
        .into_iter()
        .map(|i| i + 1)
        .collect();
    let at = |name: &str| {
        options
            .iter()
            .copied()
            .filter(|&i| argv[i] == name)
            .collect::<Vec<_>>()
    };

    // jq acts on the first help/version/build-configuration option it reads
    // and ignores the rest, so only a version reached first reports.
    let version =
        options.iter().find_map(|&i| jqargs::stop_at(&argv[i])) == Some(jqargs::Stop::Version);
    let explain = at("--explain");
    let install = at("--install-shim");
    let describe = at("--describe");
    let schema = at("--schema");
    let json = (version
        || !explain.is_empty()
        || !install.is_empty()
        || !describe.is_empty()
        || !schema.is_empty())
        && !at("--json").is_empty();

    // The bare word right after an option, if any: `--install-shim`'s
    // directory, `--schema`'s command, `--format`'s value.
    let word_after = |i: usize| {
        Some(i + 1).filter(|&j| j < argv.len() && !options.contains(&j) && argv[j] != "--")
    };

    let dir_at = install.first().and_then(|&i| word_after(i));
    let force = !install.is_empty() && !at("--force").is_empty();

    let verb = if let Some(&i) = install.first() {
        let dir = dir_at.map_or_else(|| PathBuf::from("."), |j| PathBuf::from(&argv[j]));
        (explain.is_empty() || i < explain[0]).then_some(Verb::InstallShim { dir, force })
    } else {
        None
    }
    .or_else(|| explain.first().map(|_| Verb::Explain));

    // `--describe` and `--schema` take no jq call: the first of them wins
    // over everything else, as `--version` does.
    let command_at = schema.first().and_then(|&i| word_after(i));
    let format_flag = if schema.is_empty() {
        Vec::new()
    } else {
        at("--format")
    };
    let format_at = format_flag.first().and_then(|&i| word_after(i));
    let verb = match (describe.first(), schema.first()) {
        (Some(&d), Some(&s)) if s < d => Some(Verb::Schema {
            command: command_at.map(|j| argv[j].clone()),
            format: format_at.map(|j| argv[j].clone()),
        }),
        (Some(_), _) => Some(Verb::Describe),
        (None, Some(_)) => Some(Verb::Schema {
            command: command_at.map(|j| argv[j].clone()),
            format: format_at.map(|j| argv[j].clone()),
        }),
        (None, None) => verb,
    };

    // Remove what was lifted, highest index first so the rest stay valid. Only
    // the first verb is lifted; a second one stays for the jq parser to reject.
    let mut lifted: Vec<usize> = Vec::new();
    match verb {
        Some(Verb::Explain) => lifted.push(explain[0]),
        Some(Verb::InstallShim { .. }) => {
            lifted.push(install[0]);
            lifted.extend(dir_at);
            if force {
                lifted.extend(at("--force"));
            }
        }
        Some(Verb::Describe) => lifted.push(describe[0]),
        Some(Verb::Schema { .. }) => {
            lifted.push(schema[0]);
            lifted.extend(command_at);
            lifted.extend(format_flag.first());
            lifted.extend(format_at);
        }
        None => {}
    }
    if json {
        lifted.extend(at("--json"));
    }
    lifted.sort_unstable();
    lifted.dedup();
    for i in lifted.into_iter().rev() {
        argv.remove(i);
    }
    Lifted {
        verb,
        json,
        version,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Engine, Lifted, Meta, Verb, civil_from_days, explain_json, format_utc, invoking_agent,
        shell_quote, take_verb, version_human, version_json,
    };
    use crate::translate::Plan;
    use std::ffi::OsStr;
    use std::ffi::OsString;
    use std::path::Path;
    use std::path::PathBuf;

    /// A native command line: `pathfinder` followed by `parts`.
    fn argv(parts: &[&str]) -> Vec<OsString> {
        std::iter::once("pathfinder")
            .chain(parts.iter().copied())
            .map(OsString::from)
            .collect()
    }

    #[test]
    fn explain_is_lifted_out_of_the_jq_argv() {
        let mut a = argv(["--explain", "-c", ".a"].as_ref());
        assert_eq!(take_verb(&mut a).verb, Some(Verb::Explain));
        assert_eq!(a, argv(["-c", ".a"].as_ref()));
    }

    #[test]
    fn install_shim_takes_an_optional_directory() {
        let mut a = argv(["--install-shim", "/tmp/bin"].as_ref());
        assert_eq!(
            take_verb(&mut a).verb,
            Some(Verb::InstallShim {
                dir: PathBuf::from("/tmp/bin"),
                force: false
            })
        );
        assert_eq!(a, argv(&[]));

        let mut a = argv(["--install-shim"].as_ref());
        assert_eq!(
            take_verb(&mut a).verb,
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
            take_verb(&mut a).verb,
            Some(Verb::InstallShim {
                dir: PathBuf::from("/tmp/bin"),
                force: true
            })
        );
    }

    #[test]
    fn an_ordinary_jq_argv_has_no_verb() {
        let mut a = argv(["-r", ".a", "in.json"].as_ref());
        assert_eq!(take_verb(&mut a).verb, None);
        assert_eq!(a, argv(["-r", ".a", "in.json"].as_ref()));
    }

    #[test]
    fn json_is_lifted_only_beside_a_report() {
        let mut a = argv(["--json", "--explain", "."].as_ref());
        assert_eq!(
            take_verb(&mut a),
            Lifted {
                verb: Some(Verb::Explain),
                json: true,
                version: false,
            }
        );
        assert_eq!(a, argv(["."].as_ref()));

        let mut a = argv(["--version", "--json"].as_ref());
        assert_eq!(
            take_verb(&mut a),
            Lifted {
                verb: None,
                json: true,
                version: true,
            }
        );
        assert_eq!(a, argv(["--version"].as_ref()));

        // Not a dir argument once lifted.
        let mut a = argv(["--install-shim", "--json"].as_ref());
        assert_eq!(
            take_verb(&mut a).verb,
            Some(Verb::InstallShim {
                dir: PathBuf::from("."),
                force: false
            })
        );

        // Without a report it stays, for the jq parser to reject.
        let mut a = argv(["--json", "-c", "."].as_ref());
        assert_eq!(take_verb(&mut a), Lifted::default());
        assert_eq!(a, argv(["--json", "-c", "."].as_ref()));

        // `-V` is `--version`.
        let mut a = argv(["-V", "--json"].as_ref());
        assert!(take_verb(&mut a).json);
        assert_eq!(a, argv(["-V"].as_ref()));
    }

    #[test]
    fn option_values_are_never_lifted() {
        // `--version` is `$v`'s value, so nothing reports and `--json` stays
        // for the jq parser to reject.
        let mut a = argv(["--json", "--arg", "v", "--version", "-c", "$v"].as_ref());
        assert_eq!(take_verb(&mut a), Lifted::default());
        assert_eq!(a.len(), 7);

        // `--json` is `$x`'s value: it stays bound, the verb is still lifted.
        let mut a = argv(["--explain", "--arg", "x", "--json", "-n", "$x"].as_ref());
        assert_eq!(take_verb(&mut a).verb, Some(Verb::Explain));
        assert_eq!(a, argv(["--arg", "x", "--json", "-n", "$x"].as_ref()));

        // So is a verb, and anything after `--`.
        let mut a = argv(["--arg", "x", "--explain", "-n", "$x"].as_ref());
        assert_eq!(take_verb(&mut a), Lifted::default());
        let mut a = argv(["--version", "-n", "$ARGS", "--", "--json"].as_ref());
        assert!(!take_verb(&mut a).json);
        assert_eq!(a.len(), 6);
    }

    #[test]
    fn civil_dates_match_known_days() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(59), (1970, 3, 1));
        // Leap days: an ordinary leap year, and 2000 (divisible by 400).
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
        assert_eq!(civil_from_days(11_017), (2000, 3, 1));
        // 2100 is not a leap year.
        assert_eq!(civil_from_days(47_540), (2100, 2, 28));
        assert_eq!(civil_from_days(47_541), (2100, 3, 1));
    }

    #[test]
    fn timestamps_are_iso_8601_utc() {
        assert_eq!(format_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_utc(1_709_164_800), "2024-02-29T00:00:00Z");
        assert_eq!(format_utc(951_868_799), "2000-02-29T23:59:59Z");
        assert_eq!(format_utc(1_791_676_800 + 3_723), "2026-10-11T01:02:03Z");
    }

    #[test]
    fn only_named_harnesses_label_the_report() {
        let none = |_: &str| None;
        assert_eq!(invoking_agent(none), None);
        let claude = |n: &str| (n == "CLAUDECODE").then(|| "1".into());
        assert_eq!(invoking_agent(claude), Some("claude-code"));
        let gemini = |n: &str| (n == "GEMINI_CLI").then(|| "1".into());
        assert_eq!(invoking_agent(gemini), Some("gemini-cli"));
        let empty = |n: &str| (n == "CURSOR_AGENT").then(|| "".into());
        assert_eq!(invoking_agent(empty), None);
    }

    #[test]
    fn hints_quote_unsafe_directories() {
        assert_eq!(shell_quote(Path::new("/tmp/bin")), "/tmp/bin");
        assert_eq!(shell_quote(Path::new("/tmp/my bin")), "'/tmp/my bin'");
        assert_eq!(shell_quote(Path::new("it's")), r"'it'\''s'");
    }

    fn meta() -> Meta {
        Meta {
            command: "pathfinder --version",
            timestamp: "2026-10-11T00:00:00Z".to_owned(),
            invoking_agent: None,
        }
    }

    fn engine() -> Engine<'static> {
        Engine {
            name: "jaq",
            selected_by: "PATHFINDER_ENGINE",
            jaq: Some((OsStr::new("/opt/jaq/bin/jaq"), "PATHFINDER_JAQ")),
        }
    }

    #[test]
    fn version_human_names_the_native_engine() {
        let native = Engine {
            name: "native",
            selected_by: "default",
            jaq: None,
        };
        assert!(version_human(&native).contains("\nengine: native, in process (default)\n"));
    }

    #[test]
    fn version_human_names_the_engine_and_the_maintainer() {
        assert_eq!(
            version_human(&engine()),
            format!(
                "pathfinder {} (jq-1.8.1 compatible)\n\
                 engine: jaq (PATHFINDER_ENGINE), /opt/jaq/bin/jaq (PATHFINDER_JAQ)\n\
                 Maintained by Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>\n\
                 https://Pathfinder.SpacecraftSoftware.org/\n",
                env!("CARGO_PKG_VERSION")
            )
        );
    }

    #[test]
    fn version_json_is_one_enveloped_document() {
        let text = version_json(&engine(), &meta());
        assert!(!text.contains('\n'));
        let v: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
        assert_eq!(v["metadata"]["tool"], "pathfinder");
        assert_eq!(v["metadata"]["timestamp"], "2026-10-11T00:00:00Z");
        assert!(v["metadata"].get("invoking_agent").is_none());
        assert_eq!(v["data"]["jq_baseline"], "jq-1.8.1");
        assert_eq!(v["data"]["engine"]["name"], "jaq");
        assert_eq!(v["data"]["engine"]["selected_by"], "PATHFINDER_ENGINE");
        assert_eq!(v["data"]["engine"]["path"], "/opt/jaq/bin/jaq");
        assert_eq!(v["data"]["engine"]["source"], "PATHFINDER_JAQ");
    }

    #[test]
    fn explain_json_omits_what_is_absent() {
        let plan = Plan {
            argv: vec![OsString::from("-c"), OsString::from(".")],
            cli: jaq::Cli::default(),
            post: crate::post::Post {
                ascii: false,
                seq: false,
                terminator: crate::post::Terminator::Newline,
            },
            strip_input_rs: false,
            concat_inputs: None,
            assembly: None,
        };
        let v: serde_json::Value =
            serde_json::from_str(&explain_json(&plan, crate::engine::Engine::Jaq, &meta()))
                .expect("valid JSON");
        assert_eq!(
            v["data"],
            serde_json::json!({
                "engine": "jaq",
                "jaq_argv": ["-c", "."],
                "program": {"rewritten": false},
                "execution": "exec",
                "stdout": {"ascii": false, "seq": false},
                "stdin": {"strip_rs": false},
            })
        );
    }
}
