// SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Differential tests: every case is run through **real jq** and through the
//! shim, and the two must agree on stdout bytes and exit code.
//!
//! This is the only kind of test that can actually establish compatibility.
//! Hand-written golden values encode what the author *believed* jq does, which
//! is exactly the thing under test — and in building this crate, four of the
//! author's beliefs turned out to be wrong (`--indent 0`, the invalid-JSON exit
//! code, `ltrimstr` on non-strings, and which names are jq builtins at all).
//!
//! Real jq is not installed on the development host and is not required here:
//! the suite locates it via `PATHFINDER_REAL_JQ`, or falls back to an ephemeral
//! `nix build nixpkgs#jq`, and **skips** if neither is available. Set
//! `PATHFINDER_REQUIRE_JQ=1` (as CI does) to turn that skip into a failure.
//!
//! The version is checked, not assumed. jq's own surface moves between
//! releases — `trimstr` does not exist before 1.8, `builtins` answers 218 in
//! 1.7.1 against 226 in 1.8.1 — so comparing against the wrong jq produces
//! failures that say nothing about this shim. That is not hypothetical: the
//! first CI run compared against the runner image's older jq and failed on
//! three cases for exactly this reason. Nothing is
//! installed on the host either way.

use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// A single comparison.
struct Case {
    stdin: &'static str,
    args: &'static [&'static str],
}

const fn c(stdin: &'static str, args: &'static [&'static str]) -> Case {
    Case { stdin, args }
}

/// Cases that jq and the shim must agree on, byte for byte.
///
/// Grouped by what they exercise. Several exist specifically because an earlier
/// version of the shim got them wrong.
static CASES: &[Case] = &[
    // --- core flags ---
    c(r#"{"a":1}"#, &["-c", "."]),
    c(r#"{"a":1}"#, &["."]),
    c(r#"{"a":"x"}"#, &["-r", ".a"]),
    c(r#"{"a":"x"}"#, &["-j", ".a"]),
    c(r#"["x","y"]"#, &["--raw-output0", ".[]"]),
    c(r#"{"b":1,"a":{"d":2,"c":3}}"#, &["-S", "."]),
    c(r#"{"a":1}"#, &["--tab", "."]),
    c(r#"{"a":1}"#, &["--indent", "4", "."]),
    c(r#"{"a":1}"#, &["--indent", "7", "."]),
    c("1 2", &["-c", "."]),
    c("null", &["-n", "-c", "1,2"]),
    c("a\nb", &["-R", "-c", "."]),
    c("1 2 3", &["-s", "-c", "."]),
    c(r#"{"a":1}"#, &["-c", "--unbuffered", "."]),
    c(r#"{"a":1}"#, &["-c", "-b", "."]),
    c(r#"{"a":1}"#, &["-c", "--", "."]),
    // `--indent 0` pretty-prints with no indent in BOTH; an early version of the
    // shim mistranslated it to `-c`.
    c(r#"{"a":1}"#, &["--indent", "0", "."]),
    // jq resolves -c/--tab/--indent as last-wins; jaq lets -c win regardless.
    c(r#"{"a":1}"#, &["-c", "--tab", "."]),
    c(r#"{"a":1}"#, &["--tab", "-c", "."]),
    c(r#"{"a":1}"#, &["-c", "--indent", "4", "."]),
    c(r#"{"a":1}"#, &["--indent", "4", "-c", "."]),
    c(r#"{"a":1}"#, &["--tab", "--indent", "3", "."]),
    // --- exit codes ---
    c(r#"{"a":1}"#, &["-e", ".a"]),
    c(r#"{"a":false}"#, &["-e", ".a"]),
    c("{}", &["-e", ".a"]),
    c("null", &["-ne", "empty"]),
    c("null", &["-n", r#""x"|halt_error"#]),
    c("null", &["-n", r#""m"|halt_error(7)"#]),
    c("null", &[r#"error("boom")"#]),
    c("notjson", &["-c", "."]),
    c(r#"{"a":1}"#, &["-c", ".a|.b"]),
    // --- argv grammar ---
    c(r#"{"a":9}"#, &["-c", ".", "-"]),
    c("null", &["-nc", "--arg", "a", "1", "--arg", "a", "2", "$a"]),
    c("null", &["-nc", "--arg", "x", "1", "$ARGS"]),
    // jq reads `-1` and `-5` as data here; jaq would read them as flags.
    c("null", &["-c", "-1"]),
    c("null", &["-nc", "$ARGS", "--args", "a", "-5"]),
    c("null", &["-nc", "--args", "$ARGS", "p", "q"]),
    c("null", &["-nc", "--jsonargs", "$ARGS", "1", r#"{"a":2}"#]),
    c(
        "null",
        &["-nc", "--arg", "n", "v", "--jsonargs", "$ARGS", "3"],
    ),
    // Unknown options must fail the way jq fails, including jaq's own `-i`,
    // which would otherwise rewrite the user's file in place.
    c("null", &["-Z", "."]),
    c("null", &["--nope", "."]),
    c("null", &["--arg", "x"]),
    c("null", &["-i", "."]),
    c("null", &["--argfile", "x", "f1.json", "."]),
    // --- ascii output ---
    c("null", &["-ac", r#""café""#]),
    c("null", &["-rac", r#""café""#]),
    c("null", &["-jac", r#""café""#]),
    c("null", &["-ac", r#""😀""#]),
    c("null", &["-ac", r#"{"k":"é","arr":["ü"]}"#]),
    c("null", &["-a", r#"{"k":"é"}"#]),
    c("null", &["-ac", r#""日本語""#]),
    c("null", &["-a", "--raw-output0", r#""é""#]),
    c("null", &["-aSc", r#"{"b":"é","a":1}"#]),
    c("null", &["-ac", r#""""#]),
    // --- json-seq ---
    c("\x1e1\n\x1e2\n", &["--seq", "-c", "."]),
    c("\x1e{\"a\":1}\n", &["--seq", "."]),
    c("\x1e1\n", &["--seq", "-c", ".,."]),
    // --- polyfilled builtins ---
    c(r#"{"a":[1,2],"b":{"c":3}}"#, &["-c", "[tostream]"]),
    c(
        r#"{"a":[1,2],"b":{"c":3}}"#,
        &["-c", "[fromstream(tostream)]"],
    ),
    c(
        r#"{"a":{"b":{"c":[1,2]}}}"#,
        &["-c", "[fromstream(1|truncate_stream(tostream))]"],
    ),
    c(r#"[{"a":1,"n":"x"},{"a":2,"n":"y"}]"#, &["-c", "INDEX(.a)"]),
    c(r#"[{"a":1}]"#, &["-c", "INDEX(.[];.a)"]),
    c("2", &["-c", "IN(1,2,3)"]),
    c("[1,2]", &["-c", "IN(.[];2)"]),
    c(
        r#"[{"k":1}]"#,
        &["-c", r#"JOIN({"1":{"v":9}};.k|tostring)"#],
    ),
    c(r#""xxaxx""#, &["-c", r#"trimstr("xx")"#]),
    c(r#"[1,"a"]"#, &["-c", r#"format("csv")"#]),
    c("null", &["-c", r#"format("nope")"#]),
    c("null", &["-c", "have_decnum"]),
    c("null", &["-c", "have_literal_numbers"]),
    c("null", &["-c", "builtins|length"]),
    // jq's `builtins` is unsorted; a sorted copy would answer `true` here.
    c("null", &["-c", "builtins|sort==builtins"]),
    c(r#"{"a":1}"#, &["-c", "$__loc__.line"]),
    c("null", &["-nc", "$__loc__,$__loc__"]),
    // --- setpath auto-vivification repair ---
    c("null", &["-c", r#"setpath(["a","b","c"];1)"#]),
    c("{}", &["-c", r#"setpath(["x",0,"y"];1)"#]),
    c("[1,2]", &["-c", "setpath([1];9)"]),
    c(r#"{"a":[1,2]}"#, &["-c", r#"setpath(["a",5];9)"#]),
    c(
        "null",
        &["-c", "reduce range(3) as $i (null; setpath([$i];$i))"],
    ),
    // Wrong-typed containers must still error, exactly as in jq.
    c(r#"{"a":1}"#, &["-c", "setpath([0];1)"]),
    c("[1]", &["-c", r#"setpath(["a"];1)"#]),
    // --- date repair ---
    c("1.5", &["-c", "todate"]),
    c("0", &["-c", "todate"]),
    c("1700000000", &["-c", "todate"]),
    c("1.9", &["-c", "todateiso8601"]),
    c("1700000000.7", &["-c", "todate"]),
    // --- real call sites from this workspace ---
    c(
        r#"{"url":"antigravity-cli/1.2.3-4/x"}"#,
        &[
            "-r",
            r#".url | match("antigravity-cli/([0-9.]+-[0-9]+)/").captures[0].string"#,
        ],
    ),
    c(
        r#"{"urls":[{"packagetype":"bdist_wheel","filename":"a-py3-none-any.whl","url":"u","digests":{"sha256":"s"}}]}"#,
        &[
            "-r",
            r#".urls[] | select(.packagetype=="bdist_wheel") | [(.filename | capture("py3-none-(?<tag>.+)\\.whl").tag), .url, .digests.sha256] | @tsv"#,
        ],
    ),
    c(
        r#"{"model":{"display_name":"Opus"}}"#,
        &["-r", r#""[\(.model.display_name)]""#],
    ),
    c("{}", &["-r", r#".model.display_name // "none""#]),
    c(
        r#"{"x":{}}"#,
        &[
            "-c",
            "--arg",
            "name",
            "n",
            "--argjson",
            "payload",
            r#"{"p":1}"#,
            ".x[$name] = $payload",
        ],
    ),
    // --- misc semantics ---
    c(r#"{"a":{"b":1}}"#, &["-c", "[paths]"]),
    c(r#"{"a":1}"#, &["-c", "to_entries"]),
    c(r#""abc""#, &["-c", r#"ltrimstr("a")"#]),
    c(r#""abc""#, &["-c", r#"test("B";"i")"#]),
    c("null", &["-nc", "[limit(3;repeat(1))]"]),
    c("null", &["-nc", "[1,2]-[1]"]),
];

/// Cases needing input files, which are created in a temp dir at run time.
static FILE_CASES: &[Case] = &[
    c("", &["-c", ".", "f1.json"]),
    c("", &["-c", ".", "f1.json", "f2.json"]),
    c("", &["-c", "input_filename", "f1.json"]),
    c("", &["-c", "-f", "filt.jq"]),
    c("", &["-c", "-f", "filt2.jq", "f1.json"]),
    c("", &["-nc", "--rawfile", "r", "f1.json", "$r"]),
    c("", &["-nc", "--slurpfile", "s", "f1.json", "$s"]),
    // jq treats several files as ONE stream; jaq runs the program once per
    // file. These four are the reason Pathfinder concatenates multiple inputs.
    c("", &["-s", "-c", ".", "f1.json", "f2.json"]),
    c("", &["-c", "[inputs]", "f1.json", "f2.json"]),
    c("", &["-n", "-c", "[inputs]", "f1.json", "f2.json"]),
    c("", &["-c", "input", "f1.json", "f2.json"]),
    c("", &["-c", "add", "-s", "f1.json", "f2.json"]),
    c("", &["-r", ".a", "f1.json", "f2.json"]),
    // A file whose name starts with `-` (jq: a file; jaq: a flag).
    c("", &["-c", ".", "-5"]),
];

/// The jq release this shim is written against.
const BASELINE: &str = "jq-1.8.1";

/// Locate a usable real jq, or return `None` so the suite skips rather than fails.
///
/// "Usable" includes being the baseline version: an older jq is worse than no
/// jq, because it produces confident-looking mismatches that are really just
/// version drift.
fn real_jq() -> Option<PathBuf> {
    let path = locate_jq()?;
    let out = Command::new(&path).arg("--version").output().ok()?;
    let version = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    if version == BASELINE {
        return Some(path);
    }
    eprintln!(
        "found {} at {}, but this suite is written against {BASELINE}; \
         comparing against a different release reports version drift as shim bugs",
        version,
        path.display()
    );
    None
}

/// Find a jq binary, without checking which version it is.
fn locate_jq() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("PATHFINDER_REAL_JQ") {
        return Some(PathBuf::from(p));
    }
    // Ephemeral, per `spacecraft-missing-pkg`: nothing is installed on the host.
    let out = Command::new("nix")
        .args(["build", "--no-link", "--print-out-paths", "nixpkgs#jq"])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let first = String::from_utf8(out.stdout).ok()?;
    let path = PathBuf::from(first.lines().next()?).join("bin/jq");
    path.exists().then_some(path)
}

/// Report a missing jq: a warning normally, a hard failure under
/// `PATHFINDER_REQUIRE_JQ`.
///
/// CI sets the variable, because a green tick that quietly skipped the
/// compatibility contract is worse than a red one.
fn skip_or_fail(reason: &str) {
    assert!(
        std::env::var_os("PATHFINDER_REQUIRE_JQ").is_none(),
        "PATHFINDER_REQUIRE_JQ is set but {reason}"
    );
    eprintln!("skipping: {reason}");
}

/// The shim binary, invoked under the name `jq` so `argv[0]` dispatch applies.
fn shim(dir: &std::path::Path) -> PathBuf {
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_pathfinder"));
    let link = dir.join("jq");
    if !link.exists() {
        std::os::unix::fs::symlink(&exe, &link).expect("can create the jq symlink");
    }
    link
}

fn run(exe: &std::path::Path, cwd: &std::path::Path, case: &Case) -> (Option<i32>, Vec<u8>) {
    use std::io::Write as _;
    let mut child = Command::new(exe)
        .args(case.args.iter().map(OsStr::new))
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawns");
    // A filter that never reads stdin (`-n`) closes the pipe early; that is a
    // normal outcome here, not a failure of the case.
    let _ = child
        .stdin
        .take()
        .expect("stdin piped")
        .write_all(case.stdin.as_bytes());
    let out = child.wait_with_output().expect("completes");
    (out.status.code(), out.stdout)
}

#[test]
fn shim_matches_real_jq() {
    let Some(jq) = real_jq() else {
        skip_or_fail(
            "no usable jq. Set PATHFINDER_REAL_JQ to a jq-1.8.1 binary, or make `nix` \
             available so `nix build nixpkgs#jq` can supply one ephemerally.",
        );
        return;
    };

    let dir = std::env::temp_dir().join(format!("pathfinder-diff-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(dir.join("f1.json"), r#"{"a":1}"#).expect("fixture");
    std::fs::write(dir.join("f2.json"), r#"{"a":2}"#).expect("fixture");
    std::fs::write(dir.join("filt.jq"), ".a\n").expect("fixture");
    std::fs::write(dir.join("filt2.jq"), "[tostream]\n").expect("fixture");
    std::fs::write(dir.join("-5"), "[5]").expect("fixture");
    let shim = shim(&dir);

    let mut failures: Vec<String> = Vec::new();
    for case in CASES.iter().chain(FILE_CASES) {
        let expected = run(&jq, &dir, case);
        let actual = run(&shim, &dir, case);
        if expected != actual {
            failures.push(format!(
                "  args {:?} stdin {:?}\n    jq   -> exit {:?} {:?}\n    shim -> exit {:?} {:?}",
                case.args,
                case.stdin,
                expected.0,
                String::from_utf8_lossy(&expected.1),
                actual.0,
                String::from_utf8_lossy(&actual.1),
            ));
        }
    }

    let total = CASES.len() + FILE_CASES.len();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        failures.is_empty(),
        "{} of {total} differential cases diverged from jq:\n{}",
        failures.len(),
        failures.join("\n")
    );
    eprintln!("{total} differential cases matched jq exactly");
}

/// Divergences that are known, documented, and deliberately not repaired.
///
/// Asserting on them keeps `doc/DIVERGENCES.md` honest: if jaq ever fixes one,
/// this test fails and the documentation gets updated rather than quietly
/// rotting.
#[test]
fn documented_divergences_still_diverge() {
    let Some(jq) = real_jq() else {
        skip_or_fail("no usable jq available");
        return;
    };
    let dir = std::env::temp_dir().join(format!("pathfinder-div-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let shim = shim(&dir);

    // `"a" * 0` is `""` in jq 1.8.1 and `null` in jaq. `*` is an operator, not a
    // builtin, so no definition can shadow it — repairing this needs a real
    // expression rewriter. Documented in doc/DIVERGENCES.md.
    let case = c("null", &["-nc", r#""a"*0"#]);
    let expected = run(&jq, &dir, &case);
    let actual = run(&shim, &dir, &case);
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(String::from_utf8_lossy(&expected.1).trim(), r#""""#);
    assert_eq!(String::from_utf8_lossy(&actual.1).trim(), "null");
}
