// SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Turning a parsed jq command line into the engine's run options.
//!
//! The governing rule is **whitelist, never pass through**: every option the
//! engine receives is one this module set from a jq flag it named. The
//! engine's driver is jaq's command line as a library, and jaq has options jq
//! does not (`-i/--in-place` among them); nothing of the user's command line
//! reaches the driver except through a field set here.

use std::ffi::OsString;

use crate::jqargs::{Flags, JqArgs, NamedValue, OutputFormat, PositionalMode};
use crate::post::{Post, Terminator};
use crate::prelude;
use crate::program::{self, Assembly, Wrap};

/// The internal variable prefix used for values Pathfinder injects.
///
/// Chosen to be something no reasonable filter would bind, since it briefly
/// shares the variable namespace with the user's own `--arg` names.
const INTERNAL_PREFIX: &str = "__pathfinder_";

/// What `$__loc__.file` reports for the main program.
///
/// jq answers `"<top-level>"` for an inline filter *and* for one loaded with
/// `-f`; only a filter inside an included module reports its own path.
const LOC_FILE: &str = "<top-level>";

/// Everything needed to run the engine on the user's behalf.
#[derive(Debug)]
pub struct Plan {
    /// The engine's run options. Input files that [`Plan::concat_inputs`]
    /// holds are not among its files.
    pub cli: jaq::Cli,
    /// The stdout transform (`-a`, `--seq`).
    pub post: Post,
    /// Set when `--seq` requires stdin's RS separators to be stripped.
    pub strip_input_rs: bool,
    /// Inputs read as one stream instead of passed as files. See
    /// [`concat_inputs`] for why this is ever necessary.
    pub concat_inputs: Option<Vec<OsString>>,
    /// The assembled program, when the filter had to be rewritten.
    pub assembly: Option<Assembly>,
}

/// A jq feature Pathfinder will not guess at.
#[derive(Debug, Clone)]
pub struct Unsupported {
    pub feature: String,
    pub detail: String,
}

impl Unsupported {
    fn new(feature: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            feature: feature.into(),
            detail: detail.into(),
        }
    }
}

/// Why an invocation is not run.
#[derive(Debug, Clone)]
pub enum Refusal {
    /// A jq feature the engine cannot provide.
    Unsupported(Unsupported),
    /// A program jq refuses to compile. The engine might run it, so jq's
    /// error is reported instead; `source` is the program text the span
    /// points into.
    Compile {
        error: crate::syntax::check::CompileError,
        source: String,
    },
}

impl From<Unsupported> for Refusal {
    fn from(u: Unsupported) -> Self {
        Self::Unsupported(u)
    }
}

/// Plan a parsed jq command line: the program assembled, and the engine's
/// run options.
///
/// `read_file` supplies the contents of a `-f` filter file, injected so the
/// translation stays testable without touching the filesystem.
pub fn plan(
    args: &JqArgs,
    read_file: &dyn Fn(&OsString) -> std::io::Result<String>,
) -> Result<Plan, Refusal> {
    let f = &args.flags;

    if f.stream {
        return Err(Refusal::from(Unsupported::new(
            if f.stream_errors {
                "--stream-errors"
            } else {
                "--stream"
            },
            "Pathfinder's engine has no streaming parser yet. For a filter that only \
             needs the events, `tostream` is materialising but semantically equivalent; \
             for a file too large to hold in memory there is no substitute yet.",
        )));
    }

    // `--jsonargs` values are JSON the engine's driver does not read as
    // positionals, so each is bound by name and `$ARGS` is extended.
    let (wrap, bound) = match args.positional_mode {
        PositionalMode::Json => {
            let (wrap, bound) = positional_wrap(args);
            (Some(wrap), bound)
        }
        PositionalMode::Strings | PositionalMode::Files => (None, Vec::new()),
    };

    let ctx = prelude::Context {
        input_filename: (args.files.len() == 1)
            .then(|| args.files[0].to_string_lossy().into_owned()),
        ambiguous_filename: args.files.len() > 1,
        regex_literals: Vec::new(),
        native: true,
        jaq_front_end: crate::front::selected().is_none(),
        null_input: f.null_input,
        named: args.named.iter().map(|(name, _)| name.clone()).collect(),
    };

    let source = program_source(args, read_file)?;
    // jq reports `<top-level>` for the main program regardless of whether it
    // came from argv or from `-f`; only an included module reports a path.
    let assembly = program::assemble(&source, &ctx, wrap.as_ref(), LOC_FILE);

    // jq refuses to compile before anything runs, so this comes first.
    if let Some(error) = assembly.compile_error.clone() {
        return Err(Refusal::Compile { error, source });
    }
    if let Some((name, why)) = assembly.inexpressible.first() {
        return Err(Unsupported::new(name.clone(), (*why).to_string()).into());
    }

    // Only give up `-f` when the program actually changed; an untouched filter
    // keeps the engine reading the file itself, which preserves its error
    // provenance.
    let rewritten = assembly.rewritten || wrap.is_some();
    let program = if f.from_file && !rewritten {
        jaq::Program::FromFile(args.program.clone().unwrap_or_default().into())
    } else {
        jaq::Program::Inline(assembly.text.clone())
    };
    let concat = concat_inputs(args);
    let cli = run_options(args, program, &bound, concat.is_some());

    Ok(Plan {
        cli,
        post: post_plan(f),
        strip_input_rs: f.seq,
        concat_inputs: concat,
        assembly: rewritten.then_some(assembly),
    })
}

/// Decide whether the input files must be read as one stream.
///
/// jq treats several input files as **one** stream: `jq -s . a.json b.json`
/// slurps both into a single array, and `inputs` walks across the file
/// boundary. jaq's driver runs the whole program once per file, so the same
/// command would yield two separate arrays. Reading the files as one stream
/// (`inputs::reader`), described to the driver by `Segment`s, restores jq's
/// semantics; a single file is left to the driver.
fn concat_inputs(args: &JqArgs) -> Option<Vec<OsString>> {
    (args.files.len() > 1).then(|| args.files.clone())
}

/// Decide the stdout transform from the output flags.
fn post_plan(f: &Flags) -> Post {
    let terminator = if f.ascii_output {
        // The raw modes were taken off the engine's options, so their framing
        // is reproduced here.
        if f.raw_output0 {
            Terminator::Nul
        } else if f.join_output {
            Terminator::None
        } else {
            Terminator::Newline
        }
    } else {
        // The engine applied the framing itself; leave its bytes alone.
        Terminator::Newline
    };
    Post {
        ascii: f.ascii_output,
        seq: f.seq,
        terminator,
    }
}

/// Build the `$ARGS` wrap that supplies `$ARGS.positional` for `--jsonargs`.
///
/// Each value is bound individually with `--argjson`, so the engine
/// validates them one at a time, then the wrap *extends* the engine's own
/// `$ARGS` rather than replacing it: building the object from scratch would
/// drop `$ARGS.named`, which `--arg` and `--argjson` populate.
fn positional_wrap(args: &JqArgs) -> (Wrap, Vec<(String, OsString)>) {
    let mut names: Vec<String> = Vec::with_capacity(args.positional.len());
    let mut bound = Vec::with_capacity(args.positional.len());
    for (i, value) in args.positional.iter().enumerate() {
        let name = format!("{INTERNAL_PREFIX}p{i}");
        names.push(format!("${name}"));
        bound.push((name, value.clone()));
    }
    // Extend the engine's own $ARGS rather than rebuilding it, so
    // `--arg`/`--argjson` bindings survive in `.named` — but drop the internal
    // bindings this emulation just added, which jq would never have put there.
    let wrap = Wrap(format!(
        "($ARGS | .positional = [{}] \
         | .named |= with_entries(select(.key | startswith(\"{INTERNAL_PREFIX}\") | not))) as $ARGS |",
        names.join(", ")
    ));
    (wrap, bound)
}

/// The engine's run options for a jq command line.
///
/// `bound` are the `--jsonargs` values [`positional_wrap`] binds by name;
/// `concatenated` says the input files are read as one stream
/// ([`Plan::concat_inputs`]) rather than named here. A value that is not
/// UTF-8 reaches the engine lossily converted.
fn run_options(
    args: &JqArgs,
    program: jaq::Program,
    bound: &[(String, OsString)],
    concatenated: bool,
) -> jaq::Cli {
    use jaq::Format;
    let f = &args.flags;
    let text = |s: &OsString| s.to_string_lossy().into_owned();
    let mut cli = jaq::Cli {
        null_input: f.null_input,
        slurp: f.slurp,
        sort_keys: f.sort_keys,
        color_output: f.color,
        monochrome_output: f.monochrome,
        exit_status: f.exit_status,
        filter: Some(program),
        front_end: crate::front::selected(),
        ..jaq::Cli::default()
    };
    if f.raw_input {
        cli.from = Some(Format::Raw);
    }
    // `-a` cancels the raw output modes for strings (`jq -ra '"café"'`
    // prints `"café"`, quotes included), so the engine writes JSON and the
    // framing is reapplied by `post_plan`'s transform.
    if !f.ascii_output {
        if f.raw_output0 {
            cli.to = Some(Format::Raw0);
        } else if f.join_output {
            cli.join_output = true;
            cli.to = Some(Format::Raw);
        } else if f.raw_output {
            cli.to = Some(Format::Raw);
        }
    }
    match f.output_format {
        Some(OutputFormat::Compact) => cli.compact_output = true,
        Some(OutputFormat::Tab | OutputFormat::Indent(-1)) => cli.tab = true,
        // jq's range is -1..=7, and -1 is a tab, so the conversion cannot fail.
        Some(OutputFormat::Indent(n)) => cli.indent = usize::try_from(n).ok(),
        None => {}
    }
    cli.library_path = f.lib_paths.iter().map(std::path::PathBuf::from).collect();
    for (name, value) in &args.named {
        match value {
            NamedValue::Str(v) => cli.arg.push((name.clone(), text(v))),
            NamedValue::Json(v) => cli.argjson.push((name.clone(), text(v))),
            NamedValue::RawFile(v) => cli.rawfile.push((name.clone(), v.clone())),
            NamedValue::SlurpFile(v) => cli.slurpfile.push((name.clone(), v.clone())),
        }
    }
    for (name, value) in bound {
        cli.argjson.push((name.clone(), text(value)));
    }
    if matches!(args.positional_mode, PositionalMode::Strings) {
        cli.args = args.positional.iter().map(text).collect();
    }
    // A lone input `-` is stdin, which the engine reads when it has no files.
    // Several are concatenated, `-` among them (`inputs::open`).
    if !concatenated {
        cli.files = (args.files.iter())
            .filter(|f| *f != "-")
            .map(std::path::PathBuf::from)
            .collect();
    }
    cli
}

/// Get the filter text, reading it from disk when `-f` was used.
fn program_source(
    args: &JqArgs,
    read_file: &dyn Fn(&OsString) -> std::io::Result<String>,
) -> Result<String, Unsupported> {
    let Some(program) = args.program.as_ref() else {
        // jq with no filter at all prints usage; the caller handles that before
        // reaching here, so an empty filter is the identity.
        return Ok(String::new());
    };
    if args.flags.from_file {
        read_file(program).map_err(|e| {
            Unsupported::new(
                format!("-f {}", program.to_string_lossy()),
                format!("could not read the filter file: {e}"),
            )
        })
    } else {
        program
            .to_str()
            .map(str::to_owned)
            .ok_or_else(|| Unsupported::new("filter", "jq filters must be valid UTF-8".to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::{Plan, plan};
    use crate::jqargs::parse;
    use crate::post::Terminator;
    use std::ffi::OsString;

    fn no_files(_: &OsString) -> std::io::Result<String> {
        Err(std::io::Error::other("no filesystem in tests"))
    }

    fn planned(parts: &[&str]) -> Plan {
        let raw: Vec<OsString> = parts.iter().map(OsString::from).collect();
        let parsed = parse(&raw).expect("parses");
        plan(&parsed, &no_files).expect("plans")
    }

    fn program_of(p: &Plan) -> String {
        match &p.cli.filter {
            Some(jaq::Program::Inline(text)) => text.clone(),
            other => panic!("not an inline program: {other:?}"),
        }
    }

    #[test]
    fn a_plain_invocation_maps_one_to_one() {
        let p = planned(&["-r", ".a", "in.json"]);
        assert!(matches!(p.cli.to, Some(jaq::Format::Raw)));
        assert_eq!(program_of(&p), ".a");
        assert_eq!(p.cli.files, [std::path::PathBuf::from("in.json")]);
        assert!(p.post.is_identity());
        assert!(p.assembly.is_none());
    }

    #[test]
    fn library_paths_are_kept_in_order() {
        let p = planned(&["-L/a", "-L", "/b", "."]);
        assert_eq!(
            p.cli.library_path,
            ["/a", "/b"].map(std::path::PathBuf::from)
        );
    }

    #[test]
    fn indent_minus_one_is_a_tab() {
        assert!(planned(&["--indent", "-1", "."]).cli.tab);
        assert_eq!(planned(&["--indent", "0", "."]).cli.indent, Some(0));
    }

    #[test]
    fn ascii_output_takes_the_raw_modes_off_the_engine() {
        // jq's -a cancels -r for strings, so the engine writes JSON and the
        // transform does the escaping.
        let p = planned(&["-ra", "."]);
        assert!(p.cli.to.is_none());
        assert!(p.post.ascii);
        assert_eq!(p.post.terminator, Terminator::Newline);
        assert_eq!(planned(&["-ja", "."]).post.terminator, Terminator::None);
        assert_eq!(
            planned(&["-a", "--raw-output0", "."]).post.terminator,
            Terminator::Nul
        );
    }

    #[test]
    fn args_are_positionals_dashes_included() {
        let p = planned(&["-n", "$ARGS", "--args", "a", "-5"]);
        assert_eq!(p.cli.args, ["a", "-5"]);
        assert!(p.assembly.is_none());
    }

    #[test]
    fn jsonargs_binds_each_value_and_extends_args() {
        let p = planned(&["-n", "$ARGS", "--jsonargs", "1", "{\"a\":2}"]);
        let bound: Vec<_> = p
            .cli
            .argjson
            .iter()
            .map(|(n, v)| (n.as_str(), v.as_str()))
            .collect();
        assert_eq!(
            bound,
            [("__pathfinder_p0", "1"), ("__pathfinder_p1", "{\"a\":2}")]
        );
        let program = program_of(&p);
        // Extending rather than rebuilding keeps $ARGS.named from --arg.
        assert!(program.contains(".positional = [$__pathfinder_p0, $__pathfinder_p1]"));
        assert!(
            program.contains("startswith(\"__pathfinder_\")"),
            "internal bindings must be hidden from $ARGS.named"
        );
        assert!(p.cli.files.is_empty());
    }

    #[test]
    fn a_lone_dash_is_stdin_and_several_files_are_one_stream() {
        assert!(planned(&[".", "-"]).cli.files.is_empty());
        assert_eq!(
            planned(&[".", "-5"]).cli.files,
            [std::path::PathBuf::from("-5")]
        );
        let p = planned(&["-s", ".", "a.json", "b.json"]);
        assert!(p.cli.files.is_empty());
        assert_eq!(p.concat_inputs.map(|f| f.len()), Some(2));
    }

    #[test]
    fn stream_is_refused_by_name() {
        let raw: Vec<OsString> = ["--stream", "."].iter().map(OsString::from).collect();
        let parsed = parse(&raw).expect("parses");
        let err = unsupported(plan(&parsed, &no_files).expect_err("must refuse"));
        assert_eq!(err.feature, "--stream");
        assert!(
            err.detail.contains("tostream"),
            "should point at the workaround"
        );
    }

    #[test]
    fn inexpressible_builtins_are_refused_by_name() {
        let raw: Vec<OsString> = ["modulemeta"].iter().map(OsString::from).collect();
        let parsed = parse(&raw).expect("parses");
        let err = unsupported(plan(&parsed, &no_files).expect_err("must refuse"));
        assert_eq!(err.feature, "modulemeta");
    }

    fn unsupported(r: super::Refusal) -> super::Unsupported {
        match r {
            super::Refusal::Unsupported(u) => u,
            super::Refusal::Compile { error, .. } => panic!("unexpected compile error: {error:?}"),
        }
    }

    #[test]
    fn a_program_jq_will_not_compile_is_refused_with_its_error() {
        let raw: Vec<OsString> = ["{(0):1}"].iter().map(OsString::from).collect();
        let parsed = parse(&raw).expect("parses");
        match plan(&parsed, &no_files).expect_err("must refuse") {
            super::Refusal::Compile { error, source } => {
                assert_eq!(error.message, "Cannot use number (0) as object key");
                assert_eq!(source, "{(0):1}");
            }
            super::Refusal::Unsupported(u) => panic!("wrong refusal: {u:?}"),
        }
    }

    #[test]
    fn seq_requests_stripping_on_the_way_in_too() {
        // jq's --seq is json-seq in both directions.
        let p = planned(&["--seq", "."]);
        assert!(p.post.seq);
        assert!(p.strip_input_rs);
    }
}
