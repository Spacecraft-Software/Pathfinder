// SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Turning a parsed jq command line into a jaq one.
//!
//! This is the only module that knows both dialects.
//!
//! The governing rule is **whitelist, never pass through**, and it is a safety
//! requirement rather than tidiness: jaq has `-i/--in-place`, which jq does not.
//! A shim that forwarded unrecognised flags would turn `jq -i '.' data.json`
//! from jq's `Unknown option -i` into a silent, irreversible rewrite of the
//! user's file. Every flag reaching jaq is one this module named explicitly.

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

/// Everything needed to run jaq on the user's behalf.
#[derive(Debug, Clone)]
pub struct Plan {
    /// Arguments for jaq, excluding `argv[0]`.
    pub argv: Vec<OsString>,
    /// The stdout transform; [`Post::is_identity`] decides exec versus pipe.
    pub post: Post,
    /// Set when `--seq` requires stdin's RS separators to be stripped.
    pub strip_input_rs: bool,
    /// Inputs to concatenate into jaq's stdin instead of passing as file
    /// arguments. See [`concat_inputs`] for why this is ever necessary.
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

/// Build the jaq invocation for a parsed jq command line.
///
/// `read_file` supplies the contents of a `-f` filter file, injected so the
/// translation stays testable without touching the filesystem.
#[expect(
    clippy::too_many_lines,
    reason = "one flag per branch; splitting the table would hide the 1:1 jq->jaq mapping"
)]
#[expect(
    clippy::similar_names,
    reason = "`args` (parsed jq input) and `argv` (jaq output) are the domain's own names"
)]
pub fn plan(
    args: &JqArgs,
    read_file: &dyn Fn(&OsString) -> std::io::Result<String>,
) -> Result<Plan, Unsupported> {
    let f = &args.flags;

    if f.stream {
        return Err(Unsupported::new(
            if f.stream_errors {
                "--stream-errors"
            } else {
                "--stream"
            },
            "jaq has no streaming parser. For a filter that only needs the events, \
             `tostream` is materialising but semantically equivalent; for a file too \
             large to hold in memory there is no substitute yet.",
        ));
    }

    let post = post_plan(f);
    // `-a` cancels the raw output modes for strings, so the child is asked for
    // plain JSON and the framing is reapplied here. Verified against jq 1.8.1:
    // `jq -ra '"café"'` prints `"café"`, quotes included.
    let strip_raw = f.ascii_output;

    let mut argv: Vec<OsString> = Vec::new();
    // A macro rather than a closure: the surrounding code also needs to push
    // owned `OsString`s directly, which a closure capturing `argv` would forbid.
    macro_rules! push {
        ($s:expr) => {
            argv.push(OsString::from($s))
        };
    }

    if f.null_input {
        push!("-n");
    }
    if f.raw_input {
        push!("-R");
    }
    if f.slurp {
        push!("-s");
    }
    if f.sort_keys {
        push!("-S");
    }
    if f.color {
        push!("-C");
    }
    if f.monochrome {
        push!("-M");
    }
    if f.exit_status {
        push!("-e");
    }
    if !strip_raw {
        // `--raw-output0` implies both of the others in jq, and jaq spells it
        // the same way, so emit only the most specific form.
        if f.raw_output0 {
            push!("--raw-output0");
        } else if f.join_output {
            push!("-j");
        } else if f.raw_output {
            push!("-r");
        }
    }
    // Only the winning output-format directive is forwarded: jq resolves
    // `-c`/`--tab`/`--indent` as last-wins, jaq lets `-c` win regardless, so
    // sending more than one would let jaq pick a different answer than jq.
    match f.output_format {
        Some(OutputFormat::Compact) => push!("-c"),
        // jq's `--indent -1` means tab, and jaq rejects a negative value
        // outright, so both spellings become `--tab`.
        Some(OutputFormat::Tab | OutputFormat::Indent(-1)) => push!("--tab"),
        // Every other value in jq's -1..=7 range means the same thing in both,
        // including 0, which pretty-prints with no indentation in each.
        Some(OutputFormat::Indent(n)) => {
            push!("--indent");
            argv.push(OsString::from(n.to_string()));
        }
        None => {}
    }
    for dir in &f.lib_paths {
        // Always split: jaq rejects jq's attached `-Ldir` spelling.
        push!("-L");
        argv.push(dir.clone());
    }
    for (name, value) in &args.named {
        let (flag, payload) = match value {
            NamedValue::Str(v) => ("--arg", v),
            NamedValue::Json(v) => ("--argjson", v),
            NamedValue::RawFile(v) => ("--rawfile", v),
            NamedValue::SlurpFile(v) => ("--slurpfile", v),
        };
        push!(flag);
        argv.push(OsString::from(name));
        argv.push(payload.clone());
    }

    // `--args` needs no help: jaq's is position-sensitive exactly like jq's and
    // populates `$ARGS.positional` the same way. Only `--jsonargs`, which jaq
    // lacks entirely, has to be emulated.
    let wrap = match args.positional_mode {
        PositionalMode::Strings => {
            push!("--args");
            None
        }
        PositionalMode::Json => Some(jsonargs_wrap(&mut argv, args)),
        PositionalMode::Files => None,
    };

    let ctx = prelude::Context {
        input_filename: (args.files.len() == 1)
            .then(|| args.files[0].to_string_lossy().into_owned()),
        ambiguous_filename: args.files.len() > 1,
    };

    let source = program_source(args, read_file)?;
    // jq reports `<top-level>` for the main program regardless of whether it
    // came from argv or from `-f`; only an included module reports a path.
    let assembly = program::assemble(&source, &ctx, wrap.as_ref(), LOC_FILE);

    if let Some((name, why)) = assembly.inexpressible.first() {
        return Err(Unsupported::new(name.clone(), (*why).to_string()));
    }

    // Only give up `-f` when the program actually changed; an untouched filter
    // keeps jaq reading the file itself, which preserves its error provenance.
    let rewritten = assembly.rewritten || wrap.is_some();
    if f.from_file && !rewritten {
        push!("-f");
        argv.push(args.program.clone().unwrap_or_default());
    } else {
        argv.push(OsString::from(&assembly.text));
    }

    let concat = concat_inputs(args);
    if concat.is_none() {
        argv.extend(args.files.iter().cloned());
    }
    if matches!(args.positional_mode, PositionalMode::Strings) {
        argv.extend(args.positional.iter().cloned());
    }

    Ok(Plan {
        argv,
        post,
        strip_input_rs: f.seq,
        concat_inputs: concat,
        assembly: rewritten.then_some(assembly),
    })
}

/// Decide whether the input files must be concatenated into jaq's stdin.
///
/// jq treats several input files as **one** stream: `jq -s . a.json b.json`
/// slurps both into a single array, and `inputs` walks across the file
/// boundary. jaq instead runs the whole program once per file, so the same
/// command yields two separate arrays.
///
/// Concatenating the files ourselves and feeding them to jaq on stdin restores
/// jq's semantics exactly. It costs the `exec` fast path, so it is only done
/// when there really is more than one input — the single-file and stdin cases,
/// which are the overwhelming majority, are untouched.
fn concat_inputs(args: &JqArgs) -> Option<Vec<OsString>> {
    (args.files.len() > 1).then(|| args.files.clone())
}

/// Decide the stdout transform from the output flags.
fn post_plan(f: &Flags) -> Post {
    let terminator = if f.ascii_output {
        // The raw flags were stripped from jaq's argv, so their framing effect
        // has to be reproduced here.
        if f.raw_output0 {
            Terminator::Nul
        } else if f.join_output {
            Terminator::None
        } else {
            Terminator::Newline
        }
    } else {
        // jaq applied the framing itself; leave its bytes alone.
        Terminator::Newline
    };
    Post {
        ascii: f.ascii_output,
        seq: f.seq,
        terminator,
    }
}

/// Build the `$ARGS` wrap that emulates `--jsonargs`.
///
/// Each positional is bound individually with `--argjson` so jaq validates them
/// one at a time, then the wrap *extends* jaq's own `$ARGS` rather than
/// replacing it. Building the object from scratch would silently drop
/// `$ARGS.named`, which `--arg` and `--argjson` populate.
#[expect(
    clippy::similar_names,
    reason = "`args` and `argv` are the domain's own names"
)]
fn jsonargs_wrap(argv: &mut Vec<OsString>, args: &JqArgs) -> Wrap {
    let mut names: Vec<String> = Vec::with_capacity(args.positional.len());
    for (i, value) in args.positional.iter().enumerate() {
        let name = format!("{INTERNAL_PREFIX}p{i}");
        argv.push(OsString::from("--argjson"));
        argv.push(OsString::from(&name));
        argv.push(value.clone());
        names.push(format!("${name}"));
    }
    // Extend jaq's own $ARGS rather than rebuilding it, so `--arg`/`--argjson`
    // bindings survive in `.named` — but drop the internal bindings this
    // emulation just added, which jq would never have put there.
    Wrap(format!(
        "($ARGS | .positional = [{}] \
         | .named |= with_entries(select(.key | startswith(\"{INTERNAL_PREFIX}\") | not))) as $ARGS |",
        names.join(", ")
    ))
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

    fn strs(p: &Plan) -> Vec<String> {
        p.argv
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn a_plain_invocation_maps_one_to_one() {
        let p = planned(&["-r", ".a", "in.json"]);
        assert_eq!(strs(&p), ["-r", ".a", "in.json"]);
        assert!(
            p.post.is_identity(),
            "no transform means the exec fast path"
        );
        assert!(p.assembly.is_none());
    }

    #[test]
    fn library_paths_are_always_split() {
        // jaq rejects the attached form with `unknown flag: -/`.
        let p = planned(&["-L/a", "."]);
        assert_eq!(strs(&p), ["-L", "/a", "."]);
    }

    #[test]
    fn indent_minus_one_becomes_tab() {
        assert_eq!(strs(&planned(&["--indent", "-1", "."])), ["--tab", "."]);
    }

    #[test]
    fn indent_zero_is_passed_through_unchanged() {
        // jq 1.8.1 and jaq agree that --indent 0 pretty-prints without indent.
        assert_eq!(
            strs(&planned(&["--indent", "0", "."])),
            ["--indent", "0", "."]
        );
    }

    #[test]
    fn unbuffered_and_binary_are_dropped() {
        // jaq already flushes per value, and -b is a Windows-only no-op in jq.
        assert_eq!(strs(&planned(&["--unbuffered", "-b", "."])), ["."]);
    }

    #[test]
    fn ascii_output_strips_the_raw_flags_from_the_child() {
        // jq's -a cancels -r for strings, so jaq must emit JSON and this side
        // does the escaping.
        let p = planned(&["-ra", "."]);
        assert_eq!(strs(&p), ["."]);
        assert!(p.post.ascii);
        assert_eq!(p.post.terminator, Terminator::Newline);
    }

    #[test]
    fn ascii_output_preserves_join_and_nul_framing() {
        let p = planned(&["-ja", "."]);
        assert_eq!(strs(&p), ["."]);
        assert_eq!(p.post.terminator, Terminator::None);

        let p = planned(&["-a", "--raw-output0", "."]);
        assert_eq!(p.post.terminator, Terminator::Nul);
    }

    #[test]
    fn raw_output_alone_is_forwarded_not_emulated() {
        let p = planned(&["-r", "."]);
        assert!(strs(&p).contains(&"-r".to_owned()));
        assert!(p.post.is_identity());
    }

    #[test]
    fn args_passes_straight_through() {
        // jaq's --args already matches jq's; no wrap, no rewrite.
        let p = planned(&[".", "--args", "x", "y"]);
        assert_eq!(strs(&p), ["--args", ".", "x", "y"]);
        assert!(p.assembly.is_none());
    }

    #[test]
    fn jsonargs_binds_each_value_and_extends_args() {
        let p = planned(&["-n", "$ARGS", "--jsonargs", "1", "{\"a\":2}"]);
        let a = strs(&p);
        assert!(a.contains(&"--argjson".to_owned()));
        assert!(a.contains(&"__pathfinder_p0".to_owned()));
        assert!(a.contains(&"{\"a\":2}".to_owned()));
        let program = a.last().expect("program present");
        // Extending rather than rebuilding keeps $ARGS.named from --arg.
        assert!(program.contains(".positional = [$__pathfinder_p0, $__pathfinder_p1]"));
        assert!(
            program.contains("startswith(\"__pathfinder_\")"),
            "internal bindings must be hidden from $ARGS.named"
        );
        assert!(program.ends_with("($ARGS)"));
    }

    #[test]
    fn jsonargs_values_are_not_passed_as_input_files() {
        let p = planned(&["-n", ".", "--jsonargs", "1"]);
        assert!(
            !strs(&p).contains(&"1".to_owned().clone())
                || strs(&p).iter().filter(|a| *a == "1").count() == 1
        );
    }

    #[test]
    fn a_polyfilled_filter_is_rewritten_and_reported() {
        let p = planned(&["-c", "[tostream]"]);
        let program = strs(&p).last().cloned().expect("program present");
        assert!(program.contains("def tostream:"));
        assert_eq!(
            p.assembly.expect("assembly recorded").injected,
            ["tostream"]
        );
    }

    #[test]
    fn stream_is_refused_by_name() {
        let raw: Vec<OsString> = ["--stream", "."].iter().map(OsString::from).collect();
        let parsed = parse(&raw).expect("parses");
        let err = plan(&parsed, &no_files).expect_err("must refuse");
        assert_eq!(err.feature, "--stream");
        assert!(
            err.detail.contains("tostream"),
            "should point at the workaround"
        );
    }

    #[test]
    fn inexpressible_builtins_are_refused_by_name() {
        let raw: Vec<OsString> = ["input_line_number"].iter().map(OsString::from).collect();
        let parsed = parse(&raw).expect("parses");
        let err = plan(&parsed, &no_files).expect_err("must refuse");
        assert_eq!(err.feature, "input_line_number");
    }

    #[test]
    fn seq_requests_stripping_on_the_way_in_too() {
        // jq's --seq is json-seq in both directions.
        let p = planned(&["--seq", "."]);
        assert!(p.post.seq);
        assert!(p.strip_input_rs);
    }

    /// The program sits before the input files in jaq's argv, so tests that
    /// inspect it must search rather than take the last element.
    fn program_of(p: &Plan) -> String {
        strs(p)
            .into_iter()
            .find(|a| a.contains("def ") || a.starts_with('.') || a.contains('|'))
            .expect("program present")
    }

    #[test]
    fn input_filename_is_exact_for_a_single_input() {
        let p = planned(&["input_filename", "one.json"]);
        assert!(program_of(&p).contains(r#"def input_filename: "one.json";"#));
    }

    #[test]
    fn input_filename_refuses_to_guess_across_several_inputs() {
        // Any single constant would be wrong for at least one of the inputs.
        let p = planned(&["input_filename", "a.json", "b.json"]);
        assert!(program_of(&p).contains("def input_filename: error("));
    }
}
