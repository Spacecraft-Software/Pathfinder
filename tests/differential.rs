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
    // --- assignment and deletion rewrites (src/syntax/rewrite.rs) ---
    c("null", &["-c", ".a.b.c = 1"]),
    c("{}", &["-c", ".a[2] = 1"]),
    c(r#"{"a":null}"#, &["-c", ".a.b |= 5"]),
    c(r#"{"a":[1]}"#, &["-c", ".a[] += 1"]),
    c("[1,2,3]", &["-c", ".[1,2] |= empty"]),
    c(r#"[{"v":1},{"w":{}}]"#, &["-c", "map(.w.x = 1)"]),
    c(r#"{"a":1}"#, &["-c", ".b //= 7"]),
    c(r#"{"a":1}"#, &["-c", "try (.a.b = 1) catch ."]),
    c("[1]", &["-c", "try (.a = 1) catch ."]),
    c(r#"{"a":5,"b":2}"#, &["-c", "(.a, .b) |= . * 10"]),
    c(
        r#"[{"id":1,"v":0},{"id":2,"v":0}]"#,
        &["-c", "(.[] | select(.id == 2) | .v) |= 9"],
    ),
    c(r#"{"a":1,"b":2,"c":3}"#, &["-c", "del(.a)"]),
    c(r#"{"a":1,"b":2,"c":3}"#, &["-c", "del(.c, .a)"]),
    c(r#"{"a":1,"b":2,"c":3}"#, &["-c", ".b |= empty"]),
    c(r#"[{"a":1,"b":2},{"b":3,"a":4}]"#, &["-c", "del(.[].a)"]),
    c("[0,1,2,3,4,5]", &["-c", "del(.[0,1])"]),
    c("[0,1,2,3,4,5]", &["-c", "del(.[3], .[1])"]),
    c("[0,1,2,3,4,5]", &["-c", "del(.[] | select(. % 2 == 0))"]),
    c("[0,1,2,3,4,5]", &["-c", "del(.[1:3], .[-1])"]),
    c("[0,1,2]", &["-c", "del(.[10])"]),
    c(r#"{"x":null}"#, &["-c", "del(.x.y)"]),
    c("1", &["-c", "del(.)"]),
    c(r#"[1,[1,2],{"a":1}]"#, &["-c", "del(.. | select(. == 1))"]),
    c(
        r#"{"a":{"b":1}}"#,
        &["-c", r#"try del(.a[0]) catch "error""#],
    ),
    c(
        r#"{"a":[1,2,3]}"#,
        &["-c", r#"delpaths([["a",0],["a",2]])"#],
    ),
    // --- syntax rewrites and compile-time rejections (src/syntax/) ---
    c(
        "[[3]]",
        &[
            "-c",
            r#".[] as [$a] ?// [$b] | if $a != null then error("err: \($a)") else {$a,$b} end"#,
        ],
    ),
    c(r#"{"x":1}"#, &["-c", ". as [$a] ?// $b | {$a, $b}"]),
    c(
        "[[3],[4],[5],6]",
        &["-c", ".[] | . as {a:$a} ?// {a:$a} ?// $a | $a"],
    ),
    c(
        "[1]",
        &["-c", r#"[. as [$a] ?// $b | (1, 2, error("boom"))]?"#],
    ),
    c(
        r#"{"b":[1,2]}"#,
        &["-c", ". as {$b: [$c, $d]} | [$b, $c, $d]"],
    ),
    c(
        r#"[{"a":[1]},{"a":[2]}]"#,
        &["-c", "reduce .[] as {$a: [$b]} (0; . + $b)"],
    ),
    c("null", &["-c", "{(0):1}"]),
    c("null", &["-c", ". as {(true):$foo} | $foo"]),
    c("null", &["-c", "{(1+1):2}"]),
    c("null", &["-c", "module (.+1); 0"]),
    c("null", &["-c", "module []; 0"]),
    c("null", &["-c", "module {a:1}; 0"]),
    c(r#"{"a":1}"#, &["-c", ".a?//1"]),
    c(r#"{"a":1}"#, &["-c", ".a? // 1"]),
    c(r#"{"a":1,"c":2}"#, &["-c", "{ a, $__loc__, c }"]),
    c(r#"{"a":1}"#, &["-c", "try {(.a):1} catch ."]),
    c(r#"{"a":"k"}"#, &["-c", "{(.a):1}"]),
    c(
        r#"["1", "2a", "3", " 4", "5 ", "6.7", ".89", "-876", "+5.43", 21]"#,
        &["-c", ".[] |= try tonumber"],
    ),
    c(
        r#"["", "1 2", "[1]", "null", "01", "1."]"#,
        &["-c", "[.[] | try tonumber catch .]"],
    ),
    c("[null, true, [1]]", &["-c", "[.[] | try tonumber catch .]"]),
    // --- rejecting what jq rejects, accepting what it accepts (M13) ---
    c(
        r#"["", "=", "cWl4YmF6Cg", "Not base64 data", "QUJDa", "QU=JD", "QR==", "_-", "Q"]"#,
        &["-c", "[.[] | try @base64d catch .]"],
    ),
    c(
        r#"["a%20b", "%", "%2", "%F0%93%81", "%C3%28", "%EF%BF%BD", "%FX"]"#,
        &["-c", "[.[] | try @urid catch .]"],
    ),
    c(r#""QQ""#, &["-c", r#"[format("base64d"), format("urid")]"#]),
    c(
        r#"[{"key":"a","value":1},{"Key":"b","Value":2},{"name":"c","value":3},{"Name":"d","Value":4}]"#,
        &["-c", "from_entries"],
    ),
    c(
        r#"[[{"key":null,"value":1}],[{"key":1,"value":2}],[{"key":false,"name":"x","value":2}],[]]"#,
        &["-c", "[.[] | try from_entries catch .]"],
    ),
    c(
        r#"{"a":1}"#,
        &["-c", "try with_entries(.key = null) catch ."],
    ),
    c(
        "[0,1,2]",
        &[
            "-c",
            r#"[(nan, 1.5, -1, 5, "a", [0]) as $k | try has($k) catch .]"#,
        ],
    ),
    c(
        r#"{"a":1}"#,
        &["-c", r#"[(0, "a", "b", []) as $k | try has($k) catch .]"#],
    ),
    c("null", &["-c", r#"[has(0), has("a")]"#]),
    c(
        "[-1,0,1,1114112,55296,1.9,65]",
        &["-c", "implode | explode"],
    ),
    c(
        r#"[123,["a"],[null]]"#,
        &["-c", "[.[] | try implode catch .]"],
    ),
    c(
        r#"["true","false",true," true",null,0]"#,
        &["-c", "[.[] | try toboolean catch .]"],
    ),
    // --- fractional, NaN and null indices (M11) ---
    c(
        "[0,1,2,3,4,5,6,7,8,9]",
        &["-c", "[.[1.5], .[-1.5], .[nan], .[2.0], .[length/2]]"],
    ),
    c(r#"{"a":[5,6,7],"i":1.0}"#, &["-c", ".a[.i]"]),
    c(
        "[0,1,2,3,4,5,6,7,8,9]",
        &[
            "-c",
            "[.[1.2:3.5], .[1.7:-4294967296], .[nan:2], .[7:nan], .[:-1.5]]",
        ],
    ),
    c(
        "[0,1,2,3,4]",
        &["-c", ".[1.1] = 5, del(.[1.5]), (.[1.5:3.5] = [\"x\"])"],
    ),
    c(
        "[-1, 1, 2, 3, 1000000000000000000]",
        &["-c", "map([1,2][0:.])"],
    ),
    c("[0,1,2,3,4]", &["-c", "[.[0,1:2,3]]"]),
    c(
        r#"[1,null,"abcdef",[],[1,2,3,4,5]]"#,
        &[
            "-c",
            "[.[] | .[1:3]?], [.[] | .[1:3]?] == [.[] | try .[1:3] catch empty]",
        ],
    ),
    c("null", &["-c", ".[1:3], .a[1:2], (.[1:3] = [\"x\"])"]),
    // --- regex results (M12) ---
    c(
        r#""b""#,
        &["-c", r#"[match("(?<x>a)?b?")], capture("(?<x>a)?b?")"#],
    ),
    c(r#""ac""#, &["-c", r#"[match("(a)(b)?(c)") | .captures]"#]),
    c(
        r#""ab""#,
        &["-c", r#"[match("(?<x>a)|(b)"; "g") | .captures]"#],
    ),
    c(
        r#""(x(yz""#,
        &["-c", r#"[match("[(]x\\(y(z)") | .captures]"#],
    ),
    c(
        r#"["(a)(x)?", "(b)", "c"]"#,
        &["-c", r#"[.[] as $r | "abc" | [match($r) | .captures]]"#],
    ),
    c(
        r#""ab1c""#,
        &["-c", r#"[match("[a-z]*"; "g") | [.offset, .length]]"#],
    ),
    c(
        r#""123foo456bar""#,
        &["-c", r#"gsub("[^a-z]*(?<x>[a-z]*)"; "Z\(.x)")"#],
    ),
    c(
        r#""aB""#,
        &[
            "-c",
            r#"[gsub("(?<x>.)"; "\(.x|ascii_upcase)", "\(.x|ascii_downcase)", "c")]"#,
        ],
    ),
    c(
        r#""abc""#,
        &[
            "-c",
            r#"gsub(""; "-"), [sub("a","b"; "X","Y")], [sub("a"; empty)]"#,
        ],
    ),
    c(
        r#""a, b ,c""#,
        &[
            "-c",
            r#"gsub("\\s*,\\s*"; ";"), sub("(?<x>[a-z])"; "<\(.x)>")"#,
        ],
    ),
    c(
        r#""ab""#,
        &["-c", r#"[scan("(a)|(b)")], [capture("(?<x>[a-z])"; "g")]"#],
    ),
    // --- jq's error messages (M10) ---
    c(
        r#"[{"a":[1,2]}, {"a":123}]"#,
        &["-c", "map(try .a[] catch ., .a[]?)"],
    ),
    c(
        r#"[0, 1, true, "foobar"]"#,
        &[
            "-c",
            r"[.[] | try .a catch ., try .[0] catch ., try length catch .]",
        ],
    ),
    c(
        r#"["very-long-string", "x☆☆☆☆☆", [1], null]"#,
        &["-c", "[.[] | try -. catch ., try (. - .) catch .]"],
    ),
    c(
        "[1,2,{\"a\":{\"b\":{\"c\":33}}}]",
        &["-c", r#"try join(",") catch ."#],
    ),
    c(
        "0",
        &[
            "-c",
            r#"[try (1 % .) catch ., try ({} * 2) catch ., try ("a" | floor) catch .]"#,
        ],
    ),
    c(
        r"[[], {}, 55, true]",
        &[
            "-c",
            "[.[] | try utf8bytelength catch ., try trim catch ., try bsearch(0) catch .]",
        ],
    ),
    c(
        r#"["a",1,2,3,4,5,6,7]"#,
        &[
            "-c",
            r#"[try strftime("%Y") catch ., try mktime catch ., try (0 | strftime([])) catch ., try ("x" | mktime) catch .]"#,
        ],
    ),
    c(
        "null",
        &[
            "-c",
            r#"[try error("x") catch ., try error({"a":1}) catch ., try error(null) catch .]"#,
        ],
    ),
    c(
        r#"["", "abc", "N/A", "infin", "1a", ".5", "+5", null]"#,
        &["-c", "[.[] | try tonumber catch .]"],
    ),
    c(
        "null",
        &["-c", r#"try ("foobar" | .[1.5:3.5] = "xyz") catch ."#],
    ),
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
