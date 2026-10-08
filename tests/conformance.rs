// SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Conformance: jq's *own* test suite, run through the shim.
//!
//! `tests/differential.rs` proves Pathfinder does what it was designed to do,
//! but its cases were written by the same hand that wrote the shim, and every
//! case that failed was either fixed or moved to the documented divergences.
//! That makes its 100% a statement about intent, not coverage. This suite is
//! the coverage measure: the `.test` files are jq 1.8.1's, written by jq's
//! maintainers, vendored under `tests/jq-suite/` (MIT).
//!
//! # How a case is judged
//!
//! The same way jq's own `--run-tests` runner judges it: the program runs on
//! the input line, and its outputs must equal the expected lines **as JSON
//! values** — with numbers compared as doubles, as `jv_equal` does, so `1` and
//! `1.0` are equal. A `%%FAIL` block passes only if the program fails to
//! *compile* (exit 3); a runtime error is not the rejection jq's test demands.
//!
//! # The two files beside the suite
//!
//! - `EXCLUDED` lists cases real jq 1.8.1 itself fails under this runner, as
//!   `file:line`. They need fixtures the runner does not provide (module search
//!   paths, chiefly), so they say nothing about the shim. Regenerate with
//!   `PATHFINDER_CONFORMANCE_EXE=<jq-1.8.1> PATHFINDER_CONFORMANCE_WRITE_EXCLUDED=1`.
//! - `FLOOR` is the pass count the shim must not fall below. It is a ratchet:
//!   raise it in the change that raises the score, never lower it.
//!
//! The suite needs jaq, not jq: the expected outputs are in the files. It skips
//! when jaq cannot be run, unless `PATHFINDER_REQUIRE_JAQ` is set, as CI does.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

/// How long one case may run before it counts as a failure.
///
/// Generous: real cases finish in milliseconds. jq's suite contains programs
/// that are slow on jaq (`. * 1000000000` builds a gigantic string), and a
/// hung case must cost seconds, not the whole run.
const CASE_TIMEOUT: Duration = Duration::from_secs(10);

/// Worker threads. Cases are independent processes, so this is pure wall-clock.
const WORKERS: usize = 8;

#[derive(Debug, Clone)]
enum Kind {
    /// Program must succeed and print exactly `expected`.
    Ok {
        input: String,
        expected: Vec<String>,
    },
    /// Program must fail to compile.
    Fail,
}

#[derive(Debug, Clone)]
struct Case {
    /// `file:line` of the program line; stable across runs, used by EXCLUDED.
    id: String,
    file: String,
    program: String,
    kind: Kind,
}

fn suite_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/jq-suite")
}

/// Parse one jq `.test` file with jq's runner rules: blank lines separate
/// cases, `#` lines are comments, `%%FAIL` opens a must-not-compile case.
fn parse(path: &Path) -> Vec<Case> {
    let file = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let text = std::fs::read_to_string(path).expect("suite file is readable UTF-8");
    let lines: Vec<&str> = text.split('\n').collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < lines.len() {
        let l = lines[i];
        if l.trim().is_empty() || l.starts_with('#') {
            i += 1;
            continue;
        }
        if l.starts_with("%%FAIL") {
            let program = lines.get(i + 1).copied().unwrap_or_default().to_owned();
            let id = format!("{file}:{}", i + 2);
            i += 2;
            while i < lines.len() && !lines[i].trim().is_empty() {
                i += 1;
            }
            out.push(Case {
                id,
                file: file.clone(),
                program,
                kind: Kind::Fail,
            });
            continue;
        }
        let id = format!("{file}:{}", i + 1);
        let program = l.to_owned();
        let input = lines.get(i + 1).copied().unwrap_or("null").to_owned();
        i += 2;
        let mut expected = Vec::new();
        while i < lines.len() && !lines[i].trim().is_empty() {
            expected.push(lines[i].to_owned());
            i += 1;
        }
        out.push(Case {
            id,
            file: file.clone(),
            program,
            kind: Kind::Ok { input, expected },
        });
    }
    out
}

/// Compare as jq's `jv_equal` does: every number as an `f64`.
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same(p, q))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w)))
        }
        _ => a == b,
    }
}

/// Run one program with a timeout; returns (exit code, stdout).
fn run(exe: &Path, program: &str, input: &str) -> (Option<i32>, String) {
    let mut child = Command::new(exe)
        .args(["-c", program])
        // jq's own test script runs the manual's examples with `PAGER=less`,
        // and two of them read it (`$ENV.PAGER`, `env.PAGER`). Set it here so
        // the score does not depend on the environment of whoever runs it —
        // a Nix build sandbox, for one, has no PAGER at all.
        .env("PAGER", "less")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("the shim can be spawned");
    if let Some(mut stdin) = child.stdin.take() {
        // A program that never reads input closes the pipe early; that is fine.
        let _ = stdin.write_all(input.as_bytes());
    }
    let mut stdout = child.stdout.take().expect("stdout is piped");
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout.read_to_string(&mut s);
        s
    });
    let start = Instant::now();
    let status = loop {
        if let Ok(Some(st)) = child.try_wait() {
            break Some(st);
        }
        if start.elapsed() > CASE_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(2));
    };
    let out = reader.join().unwrap_or_default();
    (status.and_then(|s| s.code()), out)
}

/// Judge one case. `Err` carries a one-line reason for the failure report.
fn judge(exe: &Path, case: &Case) -> Result<(), String> {
    match &case.kind {
        Kind::Fail => match run(exe, &case.program, "null").0 {
            Some(3) => Ok(()),
            code => Err(format!("must fail to compile; exit {code:?}")),
        },
        Kind::Ok { input, expected } => {
            let (code, out) = run(exe, &case.program, input);
            if code != Some(0) {
                return Err(format!("exit {code:?}"));
            }
            let got: Result<Vec<Value>, _> = serde_json::Deserializer::from_str(&out)
                .into_iter::<Value>()
                .collect();
            let want: Result<Vec<Value>, _> = expected
                .iter()
                .map(|e| serde_json::from_str::<Value>(e))
                .collect();
            match (got, want) {
                (Ok(g), Ok(w))
                    if g.len() == w.len() && g.iter().zip(&w).all(|(a, b)| same(a, b)) =>
                {
                    Ok(())
                }
                (Ok(g), Ok(w)) => Err(format!(
                    "got {} want {}",
                    truncate(&serde_json::to_string(&g).unwrap_or_default()),
                    truncate(&serde_json::to_string(&w).unwrap_or_default())
                )),
                (Err(e), _) => Err(format!("unparseable output: {e}")),
                (_, Err(e)) => Err(format!("unparseable expectation: {e}")),
            }
        }
    }
}

fn truncate(s: &str) -> String {
    s.chars().take(100).collect()
}

/// The executable under test: the shim invoked as `jq`, or an override.
fn exe_under_test(scratch: &Path) -> PathBuf {
    if let Some(p) = std::env::var_os("PATHFINDER_CONFORMANCE_EXE") {
        return PathBuf::from(p);
    }
    let link = scratch.join("jq");
    if !link.exists() {
        std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_pathfinder"), &link)
            .expect("can create the jq symlink");
    }
    link
}

fn read_list(name: &str) -> String {
    std::fs::read_to_string(suite_dir().join(name)).unwrap_or_default()
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one linear run-then-report sequence; splitting it scatters the exclusion and ratchet logic"
)]
fn jq_suite_conformance() {
    let scratch = std::env::temp_dir().join(format!("pathfinder-conf-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).expect("scratch dir");
    let exe = exe_under_test(&scratch);

    // Can the shim reach jaq at all? A missing engine is an environment
    // problem, not a conformance result.
    if run(&exe, ".", "1").0 == Some(127) {
        assert!(
            std::env::var_os("PATHFINDER_REQUIRE_JAQ").is_none(),
            "PATHFINDER_REQUIRE_JAQ is set but the shim cannot execute jaq"
        );
        eprintln!("skipping: the shim cannot execute jaq");
        return;
    }

    let mut files: Vec<PathBuf> = std::fs::read_dir(suite_dir())
        .expect("suite dir")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "test"))
        .collect();
    files.sort();
    let cases: Vec<Case> = files.iter().flat_map(|f| parse(f)).collect();

    // Run every case, spread across a few threads.
    let mut results: Vec<(usize, Result<(), String>)> = std::thread::scope(|s| {
        let chunks: Vec<Vec<(usize, &Case)>> = (0..WORKERS)
            .map(|w| {
                cases
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| i % WORKERS == w)
                    .collect()
            })
            .collect();
        let handles: Vec<_> = chunks
            .into_iter()
            .map(|chunk| {
                let exe = &exe;
                s.spawn(move || {
                    chunk
                        .into_iter()
                        .map(|(i, c)| (i, judge(exe, c)))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().expect("worker"))
            .collect()
    });
    results.sort_by_key(|(i, _)| *i);
    let _ = std::fs::remove_dir_all(&scratch);

    // Regenerating EXCLUDED is a separate mode, run against real jq.
    if std::env::var_os("PATHFINDER_CONFORMANCE_WRITE_EXCLUDED").is_some() {
        let failing: Vec<&str> = results
            .iter()
            .filter(|(_, r)| r.is_err())
            .map(|(i, _)| cases[*i].id.as_str())
            .collect();
        let body = format!(
            "# Cases real jq 1.8.1 fails under tests/conformance.rs: they need fixtures the\n\
             # runner does not provide. Generated, not hand-edited.\n{}\n",
            failing.join("\n")
        );
        std::fs::write(suite_dir().join("EXCLUDED"), body).expect("write EXCLUDED");
        eprintln!("wrote {} exclusions", failing.len());
        return;
    }

    let excluded: BTreeSet<String> = read_list("EXCLUDED")
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(str::to_owned)
        .collect();

    let mut per_file: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    let mut failures: Vec<String> = Vec::new();
    let (mut pass, mut total) = (0usize, 0usize);
    for (i, r) in &results {
        let c = &cases[*i];
        if excluded.contains(&c.id) {
            continue;
        }
        total += 1;
        let e = per_file.entry(c.file.as_str()).or_default();
        e.1 += 1;
        match r {
            Ok(()) => {
                pass += 1;
                e.0 += 1;
            }
            Err(why) => failures.push(format!("  {}  {}\n      {why}", c.id, truncate(&c.program))),
        }
    }

    eprintln!(
        "jq 1.8.1 conformance (tests/jq-suite, {} excluded):",
        excluded.len()
    );
    for (f, (p, t)) in &per_file {
        #[expect(clippy::cast_precision_loss, reason = "counts are far below 2^52")]
        let pct = 100.0 * *p as f64 / *t as f64;
        eprintln!("  {f:16} {p:4}/{t:<4} {pct:5.1}%");
    }
    #[expect(clippy::cast_precision_loss, reason = "counts are far below 2^52")]
    let pct = 100.0 * pass as f64 / total as f64;
    eprintln!("  {:16} {pass:4}/{total:<4} {pct:5.1}%", "TOTAL");
    if std::env::var_os("PATHFINDER_CONFORMANCE_VERBOSE").is_some() {
        eprintln!("failures:\n{}", failures.join("\n"));
    }

    let floor: usize = read_list("FLOOR")
        .lines()
        .find(|l| !l.starts_with('#') && !l.trim().is_empty())
        .and_then(|l| l.trim().parse().ok())
        .expect("tests/jq-suite/FLOOR holds a pass count");
    assert!(
        pass >= floor,
        "conformance regressed: {pass} passing, floor is {floor}. \
         Run with PATHFINDER_CONFORMANCE_VERBOSE=1 to list failures."
    );
    if pass > floor {
        eprintln!("  conformance rose above FLOOR ({floor}); raise it to {pass} in this change.");
    }
}
