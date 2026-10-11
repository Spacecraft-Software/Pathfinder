// SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A parser for **jq's** command line.
//!
//! This is hand-rolled rather than delegated to `clap`, and that is deliberate.
//! jq's grammar cannot be expressed by a declarative parser:
//!
//! - The first bare word is the *program*; every later bare word is an input
//!   file — unless `--args`/`--jsonargs` has been seen, after which later bare
//!   words become `$ARGS.positional` instead. The switch applies from where it
//!   appears, so position within argv changes an argument's meaning.
//! - Short options bundle (`-rn`), and `-L` swallows the rest of its own bundle
//!   as a value (`-rL/foo` is `-r` plus `-L /foo`).
//! - `isoptish` in jq treats `-` and `-5` as bare words, not options.
//!
//! The rules below are transcribed from jq's `src/main.c` so that the shim
//! agrees with jq on every argv it is handed, including the malformed ones.

use std::ffi::OsString;

/// A value bound to a variable by one of jq's `--arg`-family options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NamedValue {
    /// `--arg NAME VALUE`
    Str(OsString),
    /// `--argjson NAME JSON`
    Json(OsString),
    /// `--rawfile NAME FILE`
    RawFile(OsString),
    /// `--slurpfile NAME FILE`
    SlurpFile(OsString),
}

/// How to shape printed output.
///
/// jq's `-c`, `--tab` and `--indent N` all write the same internal field, so the
/// **last one on the command line wins**: `jq -c --tab` pretty-prints with tabs,
/// while `jq --tab -c` is compact. jaq resolves the same combination the other
/// way (its `-c` wins regardless of order), so the order has to be tracked here
/// and only the winner forwarded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    /// `-c`
    Compact,
    /// `--tab`
    Tab,
    /// `--indent N`, with jq's accepted range of -1..=7.
    Indent(i64),
}

/// What a bare word means at the point it is encountered.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum PositionalMode {
    /// Bare words are input filenames. jq's default.
    #[default]
    Files,
    /// `--args` was seen: bare words are `$ARGS.positional` strings.
    Strings,
    /// `--jsonargs` was seen: bare words are `$ARGS.positional` JSON texts.
    Json,
}

/// The boolean and valued switches jq accepts.
#[derive(Debug, Default, Clone)]
pub struct Flags {
    pub null_input: bool,
    pub raw_input: bool,
    pub slurp: bool,
    pub raw_output: bool,
    pub join_output: bool,
    pub raw_output0: bool,
    /// The last of `-c` / `--tab` / `--indent` to appear, or `None` for jq's
    /// default of two-space pretty printing.
    pub output_format: Option<OutputFormat>,
    pub sort_keys: bool,
    pub color: bool,
    pub monochrome: bool,
    pub exit_status: bool,
    pub ascii_output: bool,
    pub seq: bool,
    pub unbuffered: bool,
    pub binary: bool,
    pub stream: bool,
    pub stream_errors: bool,
    pub from_file: bool,
    pub help: bool,
    pub version: bool,
    pub build_configuration: bool,
    pub debug_dump_disasm: bool,
    pub debug_trace: bool,
    pub lib_paths: Vec<OsString>,
}

/// A fully parsed jq command line.
#[derive(Debug, Default)]
pub struct JqArgs {
    /// The filter text, or the filter's filename when [`Flags::from_file`] is set.
    pub program: Option<OsString>,
    pub files: Vec<OsString>,
    /// Variable bindings in argv order. jq keeps the *first* binding of a
    /// duplicated name, and so does this.
    pub named: Vec<(String, NamedValue)>,
    pub positional: Vec<OsString>,
    pub positional_mode: PositionalMode,
    pub flags: Flags,
}

/// A command line jq would itself reject.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
    /// jq prints this message without its `jq: ` prefix. Only `-L`'s missing
    /// value does, in jq 1.8.1.
    pub unprefixed: bool,
}

impl ParseError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            unprefixed: false,
        }
    }

    /// jq's `-L` error: a bare `fprintf` in its `main.c`, so no program name.
    fn library_path() -> Self {
        Self {
            message: "-L takes a parameter: (e.g. -L /search/path or -L/search/path)".to_owned(),
            unprefixed: true,
        }
    }

    /// The first line of the diagnostic, as jq prints it under the name `prog`.
    pub fn line(&self, prog: &str) -> String {
        if self.unprefixed {
            self.message.clone()
        } else {
            format!("{prog}: {}", self.message)
        }
    }
}

/// An option jq acts on the moment it reads it, ignoring the rest of argv.
///
/// jq 1.8.1 prints and exits from inside its option loop, so whatever follows
/// is never parsed: `jq --version --bogus` prints the version and exits 0. A
/// short bundle is read left to right, so `-Vh` is the version, `-hV` the
/// help, and `-zV` fails at `-z` before the `V` is reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    Help,
    Version,
    BuildConfiguration,
}

impl Flags {
    /// Whether an option has already ended parsing.
    const fn stopped(&self) -> bool {
        self.help || self.version || self.build_configuration
    }
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// jq's `isoptish`: a leading `-` followed by another `-` or an ASCII letter.
///
/// This is why `jq . -` reads from stdin and `jq 'add' -5` treats `-5` as a
/// filename rather than a bundle of unknown short options.
fn is_optish(bytes: &[u8]) -> bool {
    bytes.len() >= 2 && bytes[0] == b'-' && (bytes[1] == b'-' || bytes[1].is_ascii_alphabetic())
}

/// Parse jq's argument vector, excluding `argv[0]`.
pub fn parse(argv: &[OsString]) -> Result<JqArgs, ParseError> {
    let mut out = JqArgs::default();
    let mut args_done = false;
    let mut i = 0usize;

    while i < argv.len() {
        let arg = &argv[i];
        let bytes = arg.as_encoded_bytes();

        if !args_done && bytes == b"--" {
            args_done = true;
            i += 1;
            continue;
        }

        if args_done || !is_optish(bytes) {
            take_bare_word(&mut out, arg.clone());
            i += 1;
            continue;
        }

        // Options are ASCII by construction; a non-UTF-8 byte here is already a
        // malformed option and will fall through to the unknown-option error.
        let text = arg.to_string_lossy().into_owned();
        if bytes[1] == b'-' {
            i = parse_long(&mut out, &text[2..], argv, i)?;
        } else {
            i = parse_short_cluster(&mut out, &text[1..], argv, i)?;
        }
        if out.flags.stopped() {
            break;
        }
    }

    Ok(out)
}

/// The indices of the argv entries jq reads as options: not bare words, not the
/// values an option consumes, and nothing after `--`.
///
/// Pathfinder's native surface recognises its own flags only where jq would
/// read a flag. `--arg v --version` binds `$v` to the string `--version`; it
/// does not ask for a version, and its `--json` must not be lifted either.
/// `--args`/`--jsonargs` do not end option processing: they change only what a
/// later bare word means.
pub fn option_positions(argv: &[OsString]) -> Vec<usize> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < argv.len() {
        let bytes = argv[i].as_encoded_bytes();
        if bytes == b"--" {
            break;
        }
        if is_optish(bytes) {
            out.push(i);
            i += operand_count(bytes);
        }
        i += 1;
    }
    out
}

/// How many following argv entries the option `bytes` consumes, as [`parse`]
/// reads it. A malformed option is counted as taking none; `parse` rejects it.
fn operand_count(bytes: &[u8]) -> usize {
    match bytes {
        b"--arg" | b"--argjson" | b"--rawfile" | b"--slurpfile" => 2,
        b"--indent" | b"--library-path" => 1,
        [b'-', b'-', ..] => 0,
        // A short bundle: `L` swallows the rest of it, and takes the next entry
        // only when nothing of the bundle is left.
        [b'-', cluster @ ..] => {
            usize::from(cluster.iter().position(|&b| b == b'L') == Some(cluster.len() - 1))
        }
        _ => 0,
    }
}

/// The [`Stop`] jq acts on when it reads the option at an
/// [option position](option_positions), if any.
///
/// A short bundle is read as [`parse`] reads it: left to right, ending at `L`
/// (the rest is its path) or at a letter jq does not know (an error there).
pub fn stop_at(arg: &OsString) -> Option<Stop> {
    match arg.as_encoded_bytes() {
        b"--help" => Some(Stop::Help),
        b"--version" => Some(Stop::Version),
        b"--build-configuration" => Some(Stop::BuildConfiguration),
        [b'-', b'-', ..] => None,
        [b'-', cluster @ ..] => {
            for &b in cluster {
                match b {
                    b'h' => return Some(Stop::Help),
                    b'V' => return Some(Stop::Version),
                    // jq's other short options, which do not stop it.
                    b's' | b'r' | b'j' | b'c' | b'C' | b'M' | b'a' | b'S' | b'R' | b'n' | b'f'
                    | b'e' | b'b' => {}
                    _ => return None,
                }
            }
            None
        }
        _ => None,
    }
}

/// Assign a bare word to the program slot, an input file, or `$ARGS.positional`.
fn take_bare_word(out: &mut JqArgs, word: OsString) {
    if out.program.is_none() {
        out.program = Some(word);
        return;
    }
    match out.positional_mode {
        PositionalMode::Files => out.files.push(word),
        PositionalMode::Strings | PositionalMode::Json => out.positional.push(word),
    }
}

/// Record a variable binding, keeping the first of any duplicated name as jq does.
fn bind(out: &mut JqArgs, name: String, value: NamedValue) {
    if out.named.iter().any(|(existing, _)| *existing == name) {
        return;
    }
    out.named.push((name, value));
}

/// Fetch the `n` values following the option at `i`, or fail the way jq does.
fn values<'a>(
    argv: &'a [OsString],
    i: usize,
    n: usize,
    what: &str,
) -> Result<&'a [OsString], ParseError> {
    argv.get(i + 1..i + 1 + n)
        .ok_or_else(|| ParseError::new(what.to_owned()))
}

fn parse_long(
    out: &mut JqArgs,
    name: &str,
    argv: &[OsString],
    i: usize,
) -> Result<usize, ParseError> {
    let f = &mut out.flags;
    match name {
        "slurp" => f.slurp = true,
        "raw-output" => f.raw_output = true,
        "raw-output0" => {
            f.raw_output = true;
            f.join_output = true;
            f.raw_output0 = true;
        }
        "join-output" => {
            f.raw_output = true;
            f.join_output = true;
        }
        "compact-output" => f.output_format = Some(OutputFormat::Compact),
        "color-output" => f.color = true,
        "monochrome-output" => f.monochrome = true,
        "ascii-output" => f.ascii_output = true,
        "unbuffered" => f.unbuffered = true,
        "sort-keys" => f.sort_keys = true,
        "raw-input" => f.raw_input = true,
        "null-input" => f.null_input = true,
        "from-file" => f.from_file = true,
        "binary" => f.binary = true,
        "tab" => f.output_format = Some(OutputFormat::Tab),
        "seq" => f.seq = true,
        "stream" => f.stream = true,
        "stream-errors" => {
            f.stream = true;
            f.stream_errors = true;
        }
        "exit-status" => f.exit_status = true,
        "help" => f.help = true,
        "version" => f.version = true,
        "build-configuration" => f.build_configuration = true,
        "debug-dump-disasm" => f.debug_dump_disasm = true,
        "debug-trace" | "debug-trace=all" => f.debug_trace = true,
        "args" => out.positional_mode = PositionalMode::Strings,
        "jsonargs" => out.positional_mode = PositionalMode::Json,
        "indent" => {
            let v = values(argv, i, 1, "--indent takes one parameter")?;
            let text = v[0].to_string_lossy();
            let n: i64 = text.parse().map_err(|_unparseable| {
                ParseError::new("--indent takes a number between -1 and 7")
            })?;
            // jq's own bounds check; anything outside is a usage error there too.
            if !(-1..=7).contains(&n) {
                return Err(ParseError::new("--indent takes a number between -1 and 7"));
            }
            f.output_format = Some(OutputFormat::Indent(n));
            return Ok(i + 2);
        }
        "library-path" => {
            let v = argv.get(i + 1).ok_or_else(ParseError::library_path)?;
            f.lib_paths.push(v.clone());
            return Ok(i + 2);
        }
        // `--argfile` is deliberately absent: jq 1.8.1 removed it, and answers
        // `Unknown option --argfile`. Accepting it here would make the shim more
        // permissive than the tool it stands in for.
        "arg" | "argjson" | "rawfile" | "slurpfile" => {
            // jq's own wording names what each option's second value is.
            let second = match name {
                "arg" => "value",
                "argjson" => "text",
                _ => "filename",
            };
            let what = format!("--{name} takes two parameters (e.g. --{name} varname {second})");
            let v = values(argv, i, 2, &what)?;
            let key = v[0].to_string_lossy().into_owned();
            let value = match name {
                "arg" => NamedValue::Str(v[1].clone()),
                "argjson" => NamedValue::Json(v[1].clone()),
                "rawfile" => NamedValue::RawFile(v[1].clone()),
                _ => NamedValue::SlurpFile(v[1].clone()),
            };
            bind(out, key, value);
            return Ok(i + 3);
        }
        other => return Err(ParseError::new(format!("Unknown option --{other}"))),
    }
    Ok(i + 1)
}

fn parse_short_cluster(
    out: &mut JqArgs,
    cluster: &str,
    argv: &[OsString],
    i: usize,
) -> Result<usize, ParseError> {
    let chars: Vec<char> = cluster.chars().collect();
    let mut k = 0usize;
    while k < chars.len() {
        let f = &mut out.flags;
        match chars[k] {
            's' => f.slurp = true,
            'r' => f.raw_output = true,
            'j' => {
                f.raw_output = true;
                f.join_output = true;
            }
            'c' => f.output_format = Some(OutputFormat::Compact),
            'C' => f.color = true,
            'M' => f.monochrome = true,
            'a' => f.ascii_output = true,
            'S' => f.sort_keys = true,
            'R' => f.raw_input = true,
            'n' => f.null_input = true,
            'f' => f.from_file = true,
            'e' => f.exit_status = true,
            'b' => f.binary = true,
            // jq stops here; the rest of the bundle and of argv is never read.
            'h' => {
                f.help = true;
                return Ok(i + 1);
            }
            'V' => {
                f.version = true;
                return Ok(i + 1);
            }
            'L' => {
                // `-Ldir` consumes the rest of the bundle; a bare `-L` takes the
                // next argv entry. Either way the bundle ends here.
                let rest: String = chars[k + 1..].iter().collect();
                if rest.is_empty() {
                    let v = argv.get(i + 1).ok_or_else(ParseError::library_path)?;
                    f.lib_paths.push(v.clone());
                    return Ok(i + 2);
                }
                f.lib_paths.push(OsString::from(rest));
                return Ok(i + 1);
            }
            other => return Err(ParseError::new(format!("Unknown option -{other}"))),
        }
        k += 1;
    }
    Ok(i + 1)
}

#[cfg(test)]
mod tests {
    use super::{
        JqArgs, NamedValue, OutputFormat, ParseError, PositionalMode, Stop, option_positions,
        parse, stop_at,
    };
    use std::ffi::OsString;

    fn argv(parts: &[&str]) -> Vec<OsString> {
        parts.iter().map(OsString::from).collect()
    }

    fn ok(parts: &[&str]) -> JqArgs {
        parse(&argv(parts)).expect("command line should parse")
    }

    fn err(parts: &[&str]) -> ParseError {
        parse(&argv(parts)).expect_err("command line should be rejected")
    }

    fn program(args: &JqArgs) -> String {
        args.program
            .clone()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    }

    fn files(args: &JqArgs) -> Vec<String> {
        args.files
            .iter()
            .map(|f| f.to_string_lossy().into_owned())
            .collect()
    }

    fn positional(args: &JqArgs) -> Vec<String> {
        args.positional
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn short_flags_bundle() {
        let a = ok(&["-rn", ".a"]);
        assert!(a.flags.raw_output);
        assert!(a.flags.null_input);
        assert_eq!(program(&a), ".a");
    }

    #[test]
    fn join_output_implies_raw_output() {
        // jq sets RAW_OUTPUT alongside RAW_NO_LF for -j; a shim that forwards
        // only -j would lose the quoting behaviour.
        let a = ok(&["-j", "."]);
        assert!(a.flags.raw_output);
        assert!(a.flags.join_output);
    }

    #[test]
    fn library_path_accepts_both_spellings() {
        assert_eq!(ok(&["-L", "/a", "."]).flags.lib_paths, argv(&["/a"]));
        assert_eq!(ok(&["-L/a", "."]).flags.lib_paths, argv(&["/a"]));
        // `-L` swallows the remainder of its own bundle, so `r` is still a flag
        // but `/a` is the path, not four more flags.
        let a = ok(&["-rL/a", "."]);
        assert!(a.flags.raw_output);
        assert_eq!(a.flags.lib_paths, argv(&["/a"]));
    }

    #[test]
    fn first_bare_word_is_the_program_rest_are_files() {
        let a = ok(&[".a", "one.json", "two.json"]);
        assert_eq!(program(&a), ".a");
        assert_eq!(files(&a), ["one.json", "two.json"]);
    }

    #[test]
    fn args_switch_applies_only_to_later_bare_words() {
        // Everything before --args stays an input file.
        let a = ok(&[".", "in.json", "--args", "x", "y"]);
        assert_eq!(files(&a), ["in.json"]);
        assert_eq!(positional(&a), ["x", "y"]);
        assert_eq!(a.positional_mode, PositionalMode::Strings);
    }

    #[test]
    fn args_before_the_program_still_yields_the_program_first() {
        // jq fills the program slot from the first bare word regardless of mode.
        let a = ok(&["--args", ".", "x"]);
        assert_eq!(program(&a), ".");
        assert_eq!(positional(&a), ["x"]);
    }

    #[test]
    fn from_file_claims_the_program_slot_as_a_filename() {
        let a = ok(&["-f", "filter.jq", "in.json"]);
        assert!(a.flags.from_file);
        assert_eq!(program(&a), "filter.jq");
        assert_eq!(files(&a), ["in.json"]);
    }

    #[test]
    fn bare_dash_and_negative_numbers_are_not_options() {
        // jq's isoptish() requires `-` then `-` or a letter.
        let a = ok(&[".", "-"]);
        assert_eq!(files(&a), ["-"]);
        let b = ok(&["add", "-5"]);
        assert_eq!(files(&b), ["-5"]);
    }

    #[test]
    fn double_dash_terminates_option_processing() {
        let a = ok(&["--", "-r", "in.json"]);
        assert!(!a.flags.raw_output);
        assert_eq!(program(&a), "-r");
        assert_eq!(files(&a), ["in.json"]);
    }

    #[test]
    fn duplicate_named_arguments_keep_the_first() {
        let a = ok(&["--arg", "x", "one", "--arg", "x", "two", "."]);
        assert_eq!(a.named.len(), 1);
        assert_eq!(
            a.named[0],
            ("x".to_owned(), NamedValue::Str(OsString::from("one")))
        );
    }

    #[test]
    fn indent_is_range_checked_like_jq() {
        assert_eq!(
            ok(&["--indent", "4", "."]).flags.output_format,
            Some(OutputFormat::Indent(4))
        );
        assert_eq!(
            ok(&["--indent", "0", "."]).flags.output_format,
            Some(OutputFormat::Indent(0))
        );
        assert!(
            err(&["--indent", "8", "."])
                .message
                .contains("between -1 and 7")
        );
        assert!(
            err(&["--indent", "x", "."])
                .message
                .contains("between -1 and 7")
        );
    }

    #[test]
    fn options_missing_their_values_are_usage_errors() {
        assert!(err(&["--arg", "x"]).message.contains("two parameters"));
        assert!(err(&["--indent"]).message.contains("one parameter"));
        assert!(err(&["-L"]).message.contains("-L takes a parameter"));
    }

    #[test]
    fn unknown_options_are_reported_in_jq_wording() {
        assert_eq!(err(&["-Z", "."]).message, "Unknown option -Z");
        assert_eq!(err(&["--nope", "."]).message, "Unknown option --nope");
    }

    #[test]
    fn output_format_is_last_wins() {
        // jq resolves these by writing the same field, so order decides.
        assert_eq!(
            ok(&["-c", "--tab", "."]).flags.output_format,
            Some(OutputFormat::Tab)
        );
        assert_eq!(
            ok(&["--tab", "-c", "."]).flags.output_format,
            Some(OutputFormat::Compact)
        );
        assert_eq!(
            ok(&["-c", "--indent", "4", "."]).flags.output_format,
            Some(OutputFormat::Indent(4))
        );
        assert_eq!(
            ok(&["--indent", "4", "-c", "."]).flags.output_format,
            Some(OutputFormat::Compact)
        );
        assert_eq!(
            ok(&["--tab", "--indent", "3", "."]).flags.output_format,
            Some(OutputFormat::Indent(3))
        );
    }

    #[test]
    fn option_positions_skip_values_and_stop_at_double_dash() {
        let a = argv(&[
            "--arg",
            "v",
            "--version",
            "-L",
            "-V",
            "-rL/x",
            "--indent",
            "2",
            "--args",
            "--json",
            ".",
            "--",
            "--explain",
        ]);
        let pos: Vec<&str> = option_positions(&a)
            .into_iter()
            .map(|i| a[i].to_str().expect("UTF-8"))
            .collect();
        assert_eq!(
            pos,
            ["--arg", "-L", "-rL/x", "--indent", "--args", "--json"]
        );
        // `parse` consumes the same values: `--version` and `-V` are bound, not
        // flags, so no version is asked for.
        let parsed = ok(&[
            "--arg",
            "v",
            "--version",
            "-L",
            "-V",
            "-rL/x",
            "--indent",
            "2",
            ".",
        ]);
        assert!(!parsed.flags.version);
        assert_eq!(parsed.flags.lib_paths, argv(&["-V", "/x"]));
        assert_eq!(program(&parsed), ".");
    }

    #[test]
    fn stops_include_short_bundles_read_left_to_right() {
        for (arg, stop) in [
            ("--version", Stop::Version),
            ("-V", Stop::Version),
            ("-nV", Stop::Version),
            ("-VL/x", Stop::Version),
            ("-Vh", Stop::Version),
            ("--help", Stop::Help),
            ("-hV", Stop::Help),
            ("--build-configuration", Stop::BuildConfiguration),
        ] {
            assert_eq!(stop_at(&OsString::from(arg)), Some(stop), "{arg}");
        }
        for no in ["--versions", "-LV", "-n", "--", "V", "-rLV", "-zV"] {
            assert_eq!(stop_at(&OsString::from(no)), None, "{no}");
        }
    }

    #[test]
    fn help_and_version_end_parsing_where_jq_acts_on_them() {
        // Measured on jq 1.8.1: everything after is ignored, even an unknown
        // option or a missing value.
        assert!(ok(&["--version", "--json"]).flags.version);
        assert!(ok(&["--version", "--indent", "99"]).flags.version);
        assert!(ok(&["-n", "--version", "--arg"]).flags.version);
        assert!(ok(&["--help", "--bogus"]).flags.help);
        assert!(ok(&["-hz"]).flags.help);
        assert!(
            ok(&["--build-configuration", "--bogus"])
                .flags
                .build_configuration
        );
        let first = ok(&["-Vh"]);
        assert!(first.flags.version && !first.flags.help);
        let first = ok(&["-hV"]);
        assert!(first.flags.help && !first.flags.version);
        // What comes before is still parsed, and still fails.
        assert_eq!(err(&["-zV"]).message, "Unknown option -z");
        assert_eq!(
            err(&["--bogus", "--version"]).message,
            "Unknown option --bogus"
        );
        // As a value, `--version` is data, not a request.
        assert!(!ok(&["--arg", "v", "--version", "."]).flags.version);
    }

    #[test]
    fn option_value_messages_are_jqs() {
        assert_eq!(
            err(&["--argjson", "a"]).line("jq"),
            "jq: --argjson takes two parameters (e.g. --argjson varname text)"
        );
        assert_eq!(
            err(&["--rawfile"]).line("jq"),
            "jq: --rawfile takes two parameters (e.g. --rawfile varname filename)"
        );
        assert_eq!(
            err(&["--slurpfile", "a"]).line("jq"),
            "jq: --slurpfile takes two parameters (e.g. --slurpfile varname filename)"
        );
        assert_eq!(
            err(&["--arg"]).line("jq"),
            "jq: --arg takes two parameters (e.g. --arg varname value)"
        );
        // jq prints the `-L` error without its name.
        for form in [&["-L"][..], &["--library-path"], &["-rL"]] {
            assert_eq!(
                err(form).line("jq"),
                "-L takes a parameter: (e.g. -L /search/path or -L/search/path)"
            );
        }
    }

    #[test]
    fn stream_errors_implies_stream() {
        let a = ok(&["--stream-errors", "."]);
        assert!(a.flags.stream);
        assert!(a.flags.stream_errors);
    }
}
