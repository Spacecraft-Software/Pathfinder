// SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The native surface: `pathfinder --explain`, `--version` and
//! `--install-shim`, in human and JSON mode — and proof that none of it leaks
//! through the `jq` name.
//!
//! Every command clears the agent and CI variables first and sets back only
//! what the test is about. The development host runs under an agent
//! (`AI_AGENT`, `CLAUDECODE`) and GitHub sets `CI=true`; a test that inherited
//! either would be testing the host, not the cascade.
//!
//! `assert_cmd` pipes stdout, so the default mode seen there is the non-TTY
//! one, JSON. Human mode is checked under a pseudo-terminal from util-linux
//! `script`, and by the renderers' unit tests in `src/native.rs` where
//! `script` is unavailable.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use assert_cmd::Command;
use serde_json::Value;

/// Every variable that can change a native report.
const MODE_VARS: [&str; 6] = [
    "AI_AGENT",
    "AGENT",
    "CI",
    "CLAUDECODE",
    "CURSOR_AGENT",
    "GEMINI_CLI",
];

/// The descriptive value a real agent sets; never just `1`.
const AGENT_VALUE: &str = "claude-code_2-1-296_agent";

/// Real jq 1.8.1 for the `jq`-name comparisons, when the caller supplies one.
fn real_jq() -> Option<PathBuf> {
    std::env::var_os("PATHFINDER_REAL_JQ")
        .map(PathBuf::from)
        .filter(|p| p.exists())
}

/// A command for `program` with the mode variables cleared and `vars` set.
fn command(program: &Path, vars: &[(&str, &str)]) -> Command {
    let mut cmd = Command::new(program);
    for v in MODE_VARS {
        cmd.env_remove(v);
    }
    for (k, v) in vars {
        cmd.env(k, v);
    }
    cmd
}

fn pathfinder(vars: &[(&str, &str)]) -> Command {
    command(&assert_cmd::cargo::cargo_bin("pathfinder"), vars)
}

/// A fresh, empty directory under the system temp dir, unique per test.
fn temp_dir(tag: &str) -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "pathfinder-native-{}-{}-{tag}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("can create a temp dir");
    dir
}

/// The binary under the name `jq`, through a symlink in a temp dir.
fn jq_shim() -> PathBuf {
    let link = temp_dir("jq").join("jq");
    std::os::unix::fs::symlink(assert_cmd::cargo::cargo_bin("pathfinder"), &link)
        .expect("can create the jq symlink");
    link
}

fn stdout_of(cmd: &mut Command) -> (Option<i32>, String, String) {
    let out = cmd.output().expect("runs");
    (
        out.status.code(),
        String::from_utf8(out.stdout).expect("UTF-8 stdout"),
        String::from_utf8(out.stderr).expect("UTF-8 stderr"),
    )
}

/// Parse a report: exactly one line, one JSON document.
fn document(text: &str) -> Value {
    assert!(
        text.ends_with('\n'),
        "a report ends with a newline: {text:?}"
    );
    assert_eq!(text.matches('\n').count(), 1, "one line: {text:?}");
    serde_json::from_str(text).unwrap_or_else(|e| panic!("not JSON ({e}): {text}"))
}

fn keys(v: &Value) -> Vec<&str> {
    let mut k: Vec<&str> = v
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect();
    k.sort_unstable();
    k
}

fn assert_metadata(v: &Value, command: &str) {
    let m = &v["metadata"];
    assert_eq!(m["tool"], "pathfinder");
    assert_eq!(m["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(m["command"], command);
    assert_eq!(
        m["maintainer"],
        "Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>"
    );
    assert_eq!(m["website"], "https://Pathfinder.SpacecraftSoftware.org/");
    let ts = m["timestamp"].as_str().expect("a timestamp");
    let b = ts.as_bytes();
    assert!(
        ts.len() == 20
            && b[4] == b'-'
            && b[7] == b'-'
            && b[10] == b'T'
            && b[13] == b':'
            && b[16] == b':'
            && b[19] == b'Z',
        "ISO 8601 UTC: {ts}"
    );
}

// ---- The cascade ---------------------------------------------------------

/// `--explain` with `CI=false`: a TTY would mean human, a pipe means JSON.
#[test]
fn a_pipe_defaults_to_json() {
    let (code, out, _) = stdout_of(pathfinder(&[("CI", "false")]).args(["--explain", "."]));
    assert_eq!(code, Some(0));
    assert_metadata(&document(&out), "pathfinder --explain");
}

#[test]
fn every_cascade_step_that_means_json_gives_json() {
    for vars in [
        &[("AI_AGENT", AGENT_VALUE)][..],
        &[("AGENT", AGENT_VALUE)][..],
        &[("CI", "true")][..],
        &[("CI", "false"), ("AI_AGENT", AGENT_VALUE)][..],
    ] {
        let (code, out, _) = stdout_of(pathfinder(vars).args(["--version"]));
        assert_eq!(code, Some(0), "{vars:?}");
        assert_metadata(&document(&out), "pathfinder --version");
    }
    let (_, out, _) = stdout_of(pathfinder(&[]).args(["--json", "--version"]));
    assert_metadata(&document(&out), "pathfinder --version");
}

#[test]
fn the_invoking_agent_is_named_only_when_announced() {
    let (_, out, _) = stdout_of(pathfinder(&[]).args(["--version"]));
    assert!(document(&out)["metadata"].get("invoking_agent").is_none());
    let (_, out, _) = stdout_of(pathfinder(&[("CLAUDECODE", "1")]).args(["--version"]));
    assert_eq!(document(&out)["metadata"]["invoking_agent"], "claude-code");
}

// ---- --explain -----------------------------------------------------------

#[test]
fn explain_json_for_a_passthrough_filter() {
    let explain = |engine| {
        let vars = [("PATHFINDER_ENGINE", engine)];
        stdout_of(pathfinder(&vars).args(["--explain", "--json", "-c", ".a"]))
    };
    let (code, out, _) = explain("jaq");
    assert_eq!(code, Some(0));
    let v = document(&out);
    assert_metadata(&v, "pathfinder --explain");
    assert_eq!(keys(&v), ["data", "metadata"]);
    assert_eq!(
        v["data"],
        serde_json::json!({
            "engine": "jaq",
            "jaq_argv": ["-c", ".a"],
            "program": {"rewritten": false},
            "execution": "exec",
            "stdout": {"ascii": false, "seq": false},
            "stdin": {"strip_rs": false},
        })
    );
    let (code, out, _) = explain("native");
    assert_eq!(code, Some(0));
    let data = &document(&out)["data"];
    assert_eq!(data["engine"], "native");
    assert_eq!(data["execution"], "in-process");
}

#[test]
fn explain_json_for_a_rewritten_filter() {
    let dir = temp_dir("explain");
    for f in ["a.json", "b.json"] {
        std::fs::write(dir.join(f), "{}").expect("writes an input");
    }
    let (code, out, _) = stdout_of(
        pathfinder(&[("AI_AGENT", AGENT_VALUE), ("PATHFINDER_ENGINE", "jaq")])
            .current_dir(&dir)
            .args(["--explain", "-a", ".a.b = 1", "a.json", "b.json"]),
    );
    assert_eq!(code, Some(0));
    let data = &document(&out)["data"];
    assert_eq!(
        keys(data),
        [
            "engine",
            "execution",
            "jaq_argv",
            "program",
            "stdin",
            "stdout"
        ]
    );
    let program = &data["program"];
    assert_eq!(program["rewritten"], true);
    assert_eq!(program["syntax_rewritten"], true);
    assert!(program["text"].as_str().is_some_and(|t| t.contains(".b")));
    assert!(
        program["injected"]
            .as_array()
            .is_some_and(|a| !a.is_empty())
    );
    // Nothing shadowed, so the member is omitted rather than empty or null.
    assert!(program.get("shadowed").is_none());
    assert_eq!(data["execution"], "pipe");
    assert_eq!(data["stdout"]["ascii"], true);
    assert_eq!(data["stdin"]["concat_files"], 2);
    // The program is jaq's last argument.
    assert_eq!(
        data["jaq_argv"].as_array().and_then(|a| a.last()),
        Some(&program["text"])
    );
}

// ---- --version -----------------------------------------------------------

#[test]
fn version_json_names_the_engine() {
    let (code, out, err) = stdout_of(
        pathfinder(&[])
            .env("PATHFINDER_ENGINE", "jaq")
            .env("PATHFINDER_JAQ", "/opt/x/jaq")
            .args(["--version"]),
    );
    assert_eq!(code, Some(0));
    assert_eq!(err, "");
    let v = document(&out);
    assert_metadata(&v, "pathfinder --version");
    assert_eq!(
        v["data"],
        serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "jq_baseline": "jq-1.8.1",
            "engine": {
                "name": "jaq",
                "selected_by": "PATHFINDER_ENGINE",
                "path": "/opt/x/jaq",
                "source": "PATHFINDER_JAQ",
            },
        })
    );
    let (_, out, _) = stdout_of(
        pathfinder(&[])
            .env("PATHFINDER_ENGINE", "native")
            .args(["--version"]),
    );
    assert_eq!(
        document(&out)["data"]["engine"],
        serde_json::json!({"name": "native", "selected_by": "PATHFINDER_ENGINE"})
    );
}

#[test]
fn an_unknown_engine_is_refused_under_either_name() {
    let (code, out, err) = stdout_of(pathfinder(&[("PATHFINDER_ENGINE", "jq")]).args(["-n", "1"]));
    assert_eq!((code, out.as_str()), (Some(2), ""));
    assert_eq!(
        err,
        "pathfinder: error: PATHFINDER_ENGINE is \"jq\"; it must be \"native\" or \"jaq\"\n"
    );
    let (code, _, err) =
        stdout_of(pathfinder(&[("PATHFINDER_ENGINE", "jq")]).args(["--version", "--json"]));
    assert_eq!(code, Some(2));
    assert!(err.starts_with("{\"error\":"), "{err}");
}

#[test]
fn native_help_names_the_maintainer_on_stderr() {
    let (code, _, err) = stdout_of(pathfinder(&[]).args(["--help"]));
    assert_eq!(code, Some(0));
    assert!(err.contains("Maintained by Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>\n"));
    assert!(err.ends_with("https://Pathfinder.SpacecraftSoftware.org/\n"));
    // The native options are documented, with a `--json` example.
    for option in ["--explain", "--install-shim [DIR]", "--force", "--json"] {
        assert!(err.contains(option), "{option}");
    }
    assert!(err.contains("pathfinder --explain --json "));
}

// ---- --install-shim ------------------------------------------------------

#[test]
fn install_shim_reports_success_refusal_and_force() {
    let dir = temp_dir("install");
    let exe = assert_cmd::cargo::cargo_bin("pathfinder");

    let (code, out, _) = stdout_of(pathfinder(&[]).arg("--install-shim").arg(&dir));
    assert_eq!(code, Some(0));
    let v = document(&out);
    assert_metadata(&v, "pathfinder --install-shim");
    let link = dir.join("jq");
    assert_eq!(v["data"]["installed"], link.to_str().expect("UTF-8 path"));
    assert_eq!(
        std::fs::read_link(&link).expect("a symlink"),
        PathBuf::from(v["data"]["target"].as_str().expect("a target"))
    );
    assert_eq!(
        std::fs::canonicalize(&link).expect("resolves"),
        std::fs::canonicalize(&exe).expect("resolves")
    );

    // A jq that is not ours: a conflict, exit 5, structured error on stderr.
    std::fs::remove_file(&link).expect("removes the link");
    std::fs::write(&link, "not pathfinder").expect("writes a stand-in jq");
    let (code, out, err) = stdout_of(pathfinder(&[]).arg("--install-shim").arg(&dir));
    assert_eq!(code, Some(5));
    assert_eq!(out, "");
    let e = &document(&err)["error"];
    assert_eq!(
        keys(e),
        [
            "code",
            "command",
            "docs_url",
            "exit_code",
            "hint",
            "message",
            "timestamp"
        ]
    );
    assert_eq!(e["code"], "CONFLICT");
    assert_eq!(e["exit_code"], 5);
    assert_eq!(e["command"], "pathfinder --install-shim");
    assert_eq!(e["docs_url"], "https://Pathfinder.SpacecraftSoftware.org/");
    // The binary shell-quotes a directory with unsafe characters (a `TMPDIR`
    // with a space in it), so the hint is checked around the directory.
    let hint = e["hint"].as_str().expect("a hint");
    assert!(
        hint.starts_with("pathfinder --install-shim ") && hint.ends_with(" --force"),
        "{hint}"
    );
    assert!(hint.contains(dir.to_str().expect("UTF-8")), "{hint}");
    assert_eq!(
        std::fs::read_to_string(&link).expect("left alone"),
        "not pathfinder"
    );

    // --force replaces it.
    let (code, out, _) = stdout_of(
        pathfinder(&[])
            .arg("--install-shim")
            .arg(&dir)
            .arg("--force"),
    );
    assert_eq!(code, Some(0));
    assert_eq!(
        document(&out)["data"]["installed"],
        link.to_str().expect("UTF-8")
    );
    assert!(
        std::fs::symlink_metadata(&link)
            .expect("exists")
            .is_symlink()
    );
}

#[test]
fn install_shim_into_a_missing_directory_is_a_general_error() {
    let dir = temp_dir("missing").join("no-such-dir");
    let (code, out, err) = stdout_of(pathfinder(&[]).arg("--install-shim").arg(&dir));
    assert_eq!(code, Some(1));
    assert_eq!(out, "");
    let e = &document(&err)["error"];
    assert_eq!(e["code"], "GENERAL");
    assert_eq!(e["exit_code"], 1);
}

// ---- The jq name is unchanged --------------------------------------------

#[test]
fn jq_version_is_jqs_even_under_an_agent() {
    let jq = jq_shim();
    for vars in [
        &[("AI_AGENT", AGENT_VALUE), ("CLAUDECODE", "1")][..],
        &[("CI", "true")][..],
    ] {
        let (code, out, err) = stdout_of(command(&jq, vars).arg("--version"));
        assert_eq!(
            (code, out.as_str(), err.as_str()),
            (Some(0), "jq-1.8.1\n", "")
        );
    }
}

#[test]
fn jq_help_has_no_pathfinder_trailer() {
    let jq = jq_shim();
    let (code, _, err) = stdout_of(command(&jq, &[("AI_AGENT", AGENT_VALUE)]).arg("--help"));
    assert_eq!((code, err.as_str()), (Some(0), ""));
}

/// `jq --explain .` and `jq --json .` fail as real jq 1.8.1 fails.
///
/// Measured against jq 1.8.1: exit 2, empty stdout, and on stderr
/// `jq: Unknown option --explain` followed by the "Use jq --help" couplet,
/// whose last line has two spaces in `online docs  at`. With real jq at hand
/// the whole of stderr is compared; without it, the measured text is.
#[test]
fn jq_rejects_native_flags_as_jq_does() {
    let jq = jq_shim();
    for flag in ["--explain", "--json", "--install-shim"] {
        let mut ours = command(&jq, &[("AI_AGENT", AGENT_VALUE), ("CI", "true")]);
        let (code, out, err) = stdout_of(ours.args([flag, "."]).write_stdin("{}"));
        assert_eq!(code, Some(2), "{flag}");
        assert_eq!(out, "", "{flag}");
        assert_eq!(
            err,
            format!(
                "jq: Unknown option {flag}\n\
                 Use jq --help for help with command-line options,\n\
                 or see the jq manpage, or online docs  at https://jqlang.org\n"
            ),
            "{flag}"
        );

        if let Some(real) = real_jq() {
            let real = stdout_of(command(&real, &[]).args([flag, "."]).write_stdin("{}"));
            assert_eq!(
                (code, out.as_str(), err.as_str()),
                (real.0, real.1.as_str(), real.2.as_str()),
                "{flag}"
            );
        }
    }
}

#[test]
fn json_without_a_verb_is_still_an_unknown_option() {
    let (code, out, err) = stdout_of(
        pathfinder(&[("AI_AGENT", AGENT_VALUE)])
            .args(["--json", "-c", "."])
            .write_stdin("{}"),
    );
    assert_eq!(code, Some(2));
    assert_eq!(out, "");
    assert!(
        err.starts_with("pathfinder: Unknown option --json\n"),
        "{err}"
    );
}

#[test]
fn the_jq_name_ignores_the_mode_entirely() {
    // The same filter through both names: the jq name's output must not be
    // reshaped by an agent environment. `-n` keeps it independent of input.
    let jq = jq_shim();
    let run = |vars: &[(&str, &str)]| stdout_of(command(&jq, vars).args(["-n", "-c", "{a:1}"]));
    let plain = run(&[]);
    if plain.0 == Some(127) {
        // Both runs would fail alike without a jaq, proving nothing.
        assert!(
            std::env::var_os("PATHFINDER_REQUIRE_JAQ").is_none(),
            "PATHFINDER_REQUIRE_JAQ is set but the shim cannot execute jaq"
        );
        eprintln!("skipping: the shim cannot execute jaq");
        return;
    }
    assert_eq!(plain, (Some(0), "{\"a\":1}\n".to_owned(), String::new()));
    let agent = run(&[
        ("AI_AGENT", AGENT_VALUE),
        ("CI", "true"),
        ("CLAUDECODE", "1"),
    ]);
    assert_eq!(plain, agent);
}

#[test]
fn json_given_as_an_option_value_is_not_lifted() {
    // `--version` is `$v`'s value, not a flag: nothing reports, and `--json`
    // is the unknown option jq would reject.
    let (code, out, err) = stdout_of(
        pathfinder(&[])
            .args(["--json", "--arg", "v", "--version", "-c", "$v"])
            .write_stdin("{}"),
    );
    assert_eq!(code, Some(2));
    assert_eq!(out, "");
    assert!(
        err.starts_with("pathfinder: Unknown option --json\n"),
        "{err}"
    );

    // `--json` as `$x`'s value stays bound to `$x`.
    let (code, out, _) =
        stdout_of(pathfinder(&[]).args(["--explain", "--arg", "x", "--json", "-n", "$x"]));
    assert_eq!(code, Some(0));
    let argv = &document(&out)["data"]["jaq_argv"];
    assert!(
        argv.as_array()
            .is_some_and(|a| a.iter().any(|v| v == "--json")),
        "{argv}"
    );
}

#[test]
fn short_version_takes_json_too() {
    let (code, out, _) = stdout_of(pathfinder(&[]).args(["-V", "--json"]));
    assert_eq!(code, Some(0));
    assert_metadata(&document(&out), "pathfinder --version");
}

// ---- Failures of a report, in JSON mode ------------------------------------

/// Run `args` natively and return the parsed `{"error":…}` from stderr,
/// checking the exit code and that stdout stayed empty.
fn json_error(cmd: &mut Command, exit: i32) -> Value {
    let (code, out, err) = stdout_of(cmd);
    assert_eq!(code, Some(exit), "{err}");
    assert_eq!(out, "");
    let v = document(&err);
    assert_eq!(v["error"]["exit_code"], exit);
    assert_eq!(
        v["error"]["docs_url"],
        "https://Pathfinder.SpacecraftSoftware.org/"
    );
    assert!(v["error"]["timestamp"].is_string());
    v["error"].clone()
}

#[test]
fn explain_failures_are_structured_in_json_mode() {
    let e = json_error(
        pathfinder(&[]).args(["--explain", "--json", "-n", "{(1):2}"]),
        3,
    );
    assert_eq!(e["code"], "COMPILE_ERROR");
    assert_eq!(e["command"], "pathfinder --explain");
    assert_eq!(
        e["message"],
        "Cannot use number (1) as object key at <top-level>, line 1, column 3"
    );

    // The same in JSON mode by the cascade alone: stdout is a pipe.
    let e = json_error(pathfinder(&[]).args(["--explain", "-n", "{(1):2}"]), 3);
    assert_eq!(e["code"], "COMPILE_ERROR");

    let e = json_error(
        pathfinder(&[]).args(["--explain", "--indent", "99", "."]),
        2,
    );
    assert_eq!(e["code"], "INVALID_ARGUMENT");
    assert_eq!(e["hint"], "pathfinder --help");

    // Off a terminal jq runs `.` when no filter is given, so only `-f` with no
    // file is still missing one.
    let e = json_error(
        pathfinder(&[("AI_AGENT", AGENT_VALUE)]).args(["--explain", "-f"]),
        2,
    );
    assert_eq!(e["code"], "MISSING_ARGUMENT");

    let e = json_error(pathfinder(&[]).args(["--explain", "--stream", "."]), 2);
    assert_eq!(e["code"], "UNSUPPORTED");

    // An unknown option before `--version` fails; after it, it is never read.
    let e = json_error(pathfinder(&[]).args(["--bogus", "--version"]), 2);
    assert_eq!(e["command"], "pathfinder --version");
    assert_eq!(e["message"], "Unknown option --bogus");
    let (code, out, _) = stdout_of(pathfinder(&[]).args(["--version", "--bogus", "--json"]));
    assert_eq!(code, Some(0));
    assert_metadata(&document(&out), "pathfinder --version");
}

#[test]
fn explain_without_a_filter_off_a_terminal_explains_dot() {
    let (code, out, _) = stdout_of(pathfinder(&[]).args(["--explain", "-c"]));
    assert_eq!(code, Some(0));
    let argv = &document(&out)["data"]["jaq_argv"];
    assert_eq!(
        argv.as_array().and_then(|a| a.last()),
        Some(&Value::from(".")),
        "{argv}"
    );
}

#[test]
fn explain_failures_keep_jqs_text_in_human_mode() {
    let Some((code, out)) = in_tty("--explain -n '{(1):2}'", &[]) else {
        return;
    };
    assert_eq!(code, Some(3));
    assert_eq!(
        out,
        "pathfinder: error: Cannot use number (1) as object key at <top-level>, line 1, column 3:\n    \
         {(1):2}\n      ^\npathfinder: 1 compile error\n"
    );
}

#[test]
fn install_shim_hints_change_something() {
    // `DIR` is a file: the hint points at its directory, not at itself.
    let dir = temp_dir("not-a-dir");
    let file = dir.join("afile");
    std::fs::write(&file, "").expect("writes a file");
    let e = json_error(pathfinder(&[]).arg("--install-shim").arg(&file), 1);
    assert_eq!(e["code"], "GENERAL");
    let hint = e["hint"].as_str().expect("a hint");
    assert!(!hint.contains("afile"), "{hint}");
    assert!(hint.starts_with("pathfinder --install-shim "), "{hint}");
}

// ---- Human mode, under a real terminal -----------------------------------

/// Run the native binary with stdout on a pseudo-terminal, through
/// util-linux `script`; `None` where `script` is unavailable.
///
/// The terminal merges stdout and stderr and turns `\n` into `\r\n`; the
/// latter is undone here.
fn in_tty(args: &str, vars: &[(&str, &str)]) -> Option<(Option<i32>, String)> {
    let script = Path::new("/usr/bin/script");
    if !script.exists() {
        eprintln!("skipping: no util-linux script for a pseudo-terminal");
        return None;
    }
    let exe = assert_cmd::cargo::cargo_bin("pathfinder");
    let line = format!("'{}' {args}", exe.display());
    let (code, out, _) = stdout_of(command(script, vars).args(["-qec", &line, "/dev/null"]));
    Some((code, out.replace("\r\n", "\n")))
}

#[test]
fn a_terminal_gets_the_human_version() {
    let vars = [
        ("PATHFINDER_ENGINE", "jaq"),
        ("PATHFINDER_JAQ", "/opt/x/jaq"),
    ];
    let Some((code, out)) = in_tty("--version", &vars) else {
        return;
    };
    assert_eq!(code, Some(0));
    assert_eq!(
        out,
        format!(
            "pathfinder {} (jq-1.8.1 compatible)\n\
             engine: jaq (PATHFINDER_ENGINE), /opt/x/jaq (PATHFINDER_JAQ)\n\
             Maintained by Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>\n\
             https://Pathfinder.SpacecraftSoftware.org/\n",
            env!("CARGO_PKG_VERSION")
        )
    );
    // CI=false is not CI: still the terminal's choice.
    let Some((_, out)) = in_tty("--version", &[("CI", "false")]) else {
        return;
    };
    assert!(out.starts_with("pathfinder "), "{out}");
    // The agent outranks the terminal.
    let Some((_, out)) = in_tty("--version", &[("AI_AGENT", AGENT_VALUE)]) else {
        return;
    };
    assert!(out.starts_with('{'), "{out}");
}

#[test]
fn a_terminal_gets_the_human_explain_and_refusal() {
    let Some((code, out)) = in_tty("--explain -c .", &[("PATHFINDER_ENGINE", "jaq")]) else {
        return;
    };
    assert_eq!(code, Some(0));
    assert_eq!(
        out,
        "engine: jaq\n\njaq argv (the jaq engine's command line):\n  -c\n  .\n\n\
         program: passed through unchanged\n\nexecution: exec (no extra process)\n"
    );

    let dir = temp_dir("tty-install");
    std::fs::write(dir.join("jq"), "not pathfinder").expect("writes a stand-in jq");
    let Some((code, out)) = in_tty(&format!("--install-shim '{}'", dir.display()), &[]) else {
        return;
    };
    assert_eq!(code, Some(5));
    assert_eq!(
        out,
        format!(
            "pathfinder: {} already exists. Re-run with --force to replace it.\n",
            dir.join("jq").display()
        )
    );
}
