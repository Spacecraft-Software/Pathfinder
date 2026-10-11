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
    // --- no filter: off a terminal jq runs `.` ---
    c(r#"{"a":1}"#, &[]),
    c(r#"{"a":1}"#, &["-c"]),
    c("", &["-n"]),
    c("", &["-f"]),
    // --- help, version and build configuration end option parsing ---
    c("", &["--version", "--json"]),
    c("", &["--version", "--bogus"]),
    c("", &["-n", "--version", "--arg"]),
    c("", &["-Vz"]),
    c("", &["-Vh"]),
    c("", &["-zV"]),
    c("", &["--bogus", "--version"]),
    c("", &["--arg", "v", "--version", "-n", "$v"]),
    c("", &["--help", "--bogus"]),
    c("", &["-hz"]),
    c("", &["--indent", "99", "--version"]),
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
    // `.a.[0]` is jq 1.7+ syntax that jaq 3.0 cannot parse; it is rewritten.
    c(r#"{"a":[1,2]}"#, &["-c", ".a.[0], [.a.[]], (.a.[1] = 9)"]),
    // --- division, modulo and number syntax (M14) ---
    c(
        "0",
        &[
            "-c",
            "[try (1 / .) catch ., try (0 / 0) catch ., try (5 % 0.4) catch ., try (1 % .) catch .]",
        ],
    ),
    c("[2,0,-4]", &["-c", "[.[] | (1 / .)?]"]),
    c(
        "null",
        &[
            "-c",
            r#"[5 % 2, -5 % 2, 5 % -2, 5 % -1], ("a,b" / ","), [(1,2) / (10,20)]"#,
        ],
    ),
    c(r#"{"a":8}"#, &["-c", "try (.a /= 0) catch ., (.a %= 3)"]),
    c("0.7", &["-c", "select(. > .5), [.5, -.5]"]),
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
    // --- operand order: jq evaluates the right operand in the outer loop ---
    c("null", &["-nc", "[(1,2) + (10,20)]"]),
    c("null", &["-nc", "[(1,2) - (10,20)]"]),
    c("null", &["-nc", "[(1,2) * (10,100)]"]),
    c("null", &["-nc", r#"[("a","b") + ("c","d")]"#]),
    c("null", &["-nc", "[(1,2) == (2,3)]"]),
    c("null", &["-nc", "[(1,2) != (2,3)]"]),
    c("null", &["-nc", "[(1,2) < (1,2), (1,2) <= (1,2)]"]),
    c("null", &["-nc", "[(1,2) > (1,2), (1,2) >= (1,2)]"]),
    c("null", &["-nc", "[(1,2) + (10,20) + (100,200)]"]),
    c("null", &["-nc", "[limit(3; (1,2,3) + (10,20))]"]),
    c("null", &["-nc", r#""x\((1,2) + (10,20))""#]),
    c(r#"{"a":[1,2],"b":[3,4]}"#, &["-c", "[.a[] + .b[]]"]),
    c("1 2 3", &["-c", "[input - input]"]),
    c("1 2 3", &["-c", "input + (10,20)"]),
    c("1", &["-c", "try (.a + .b) catch ."]),
    c("1", &["-c", "try (.a < .b) catch ."]),
    c(r#"{"a":"x","b":"y"}"#, &["-c", "{(.a + .b): 1}"]),
    c("[1,2,3,4]", &["-c", "del(.[.[0] + .[1]])"]),
    c(r#"{"a":1,"b":2,"c":3}"#, &["-c", ".a |= .b + .c"]),
    c("[1,2]", &["-c", "[.[] * .[]]"]),
    // --- remainder errors: a type error before a zero divisor ---
    c(
        "{}",
        &[
            "-c",
            r#"[(null, "a", [], 1) | try (. % 0) catch .], [try ({} % 0.5) catch .]"#,
        ],
    ),
    c(
        r#"{"x":5,"y":null}"#,
        &[
            "-c",
            "[try (.x %= 0) catch ., try (.y %= 0) catch ., (.x %= 2)]",
        ],
    ),
];

/// Cases only the packaged, patched jaq (`packaging/jaq/`) answers as jq does:
/// number output, double-precision integers, input literals, string
/// repetition. They run when the engine's `--version` says `+pathfinder`.
static PATCHED_CASES: &[Case] = &[
    c(
        "null",
        &[
            "-c",
            "[4 / 2, ([1,2,3] | add / length), 1.5 * 2, (9 | sqrt)]",
        ],
    ),
    c(
        "null",
        &[
            "-c",
            "[1e3, 1e17 * 1, 1e-7 * 1, nan, infinite, -infinite, 0 * -1]",
        ],
    ),
    c(
        "[1E+1000, 1.000, -0, 0.0000001, 12345678901234567890]",
        &["-c", "., map(. + 0), map(tostring)"],
    ),
    c("13911860366432393", &["-c", ". - 10, (. + 0 | tostring)"]),
    c("[nan, -NaN, Infinity, -infinity, inf]", &["-c", "."]),
    c("\u{feff}{\"bom\": true}", &["-c", "."]),
    c(
        "[-1, -0.5, 0, 0.5, 1.5, 3.7]",
        &["-c", r#"[.[] * "ab"], [try ("abc" * 1000000000) catch .]"#],
    ),
    c(
        "[1, 2.5]",
        &["-c", "map(tostring), (.[1] * 2), (. | tojson)"],
    ),
    // `0 * -1` is -0, which jq takes as 0 wherever it needs an integer.
    c(
        "[1,2]",
        &[
            "-c",
            "(0 * -1) as $z | [.[$z], .[$z:], del(.[$z]), ([$z] | implode), \
             (1 | significand), ldexp(1; $z), ($z | tostring)]",
        ],
    ),
    // `nan % 0` is NaN, which only the patched engine prints as `null`.
    c("null", &["-c", "nan % 0"]),
    // An object is indexed only with a string.
    c(
        r#"{"a":1}"#,
        &[
            "-c",
            "[(3, null, true, [1], {}, nan) as $k | try .[$k] catch .], [.[3]?]",
        ],
    ),
    c(
        r#"{"a":1}"#,
        &[
            "-c",
            "[try (.[3] = 1) catch ., try (.[3] |= 1) catch ., try del(.[3]) catch ., \
             try getpath([3]) catch ., try setpath([3]; 1) catch ., try delpaths([[3]]) catch ., \
             try has(3) catch ., try path(.[3]) catch ., try .[3:4] catch .]",
        ],
    ),
];

/// Patched-engine cases compared on stderr too: an uncaught error is reported
/// as jq reports it, with its location, and the next input still runs; the
/// exit status is the last input's. Also `debug` and `halt_error` output.
static PATCHED_STDERR_CASES: &[Case] = &[
    c("1 2", &["-c", ".a"]),
    c(r#"{"a":1} 1"#, &["-c", ".a"]),
    c(r#"1 {"a":1}"#, &["-c", ".a"]),
    c("1\n2\n{\"a\":3}\n", &["-c", ".a"]),
    c("1 \n2\n\n[1,\n2]", &["-c", ".a"]),
    c("1\r\n2", &["-c", ".a"]),
    c("1\n2\n", &["-R", ".a"]),
    c("1\n2\n", &["-s", ".a"]),
    c("1\n2\n", &["-n", "input | .a"]),
    c("1", &["-n", ".a"]),
    c("1 2", &["-c", "3, .a"]),
    c("1 2", &["-e", "if . == 1 then .a else empty end"]),
    c("1 {}", &["-e", ".a"]),
    c("null 2", &["-e", "if . == 2 then .a else . end"]),
    c(r#""a" 1 null {"message":"m"} 1.0"#, &["-c", "error"]),
    c("1", &["-c", r#"error("a\u0000b")"#]),
    c(
        r#"[1, "abcdefghijklmnop", {"a":true}, "x", {"a":1}, {}]"#,
        &["-c", ".[] | (.[], (.a | length), . - 1, .[3], .[1:2])"],
    ),
    c(r#"{"a":1}"#, &["-c", ".[3] = 1"]),
    c("null", &["-c", r#"[1, {"a": 2}] | debug | empty"#]),
    c("null", &["-c", r#""x" | debug("m", 1) | empty"#]),
    c("1", &["-c", "halt_error"]),
    c("null", &["-c", r#"{"a": [1, 2]} | halt_error(3)"#]),
    c("null", &["-c", "halt_error"]),
    c("null", &["-c", r#""x\n" | halt_error"#]),
    // A path expression that lost its path fails where and as jq's does
    // (`packaging/jaq/0010`, `0011`).
    c(
        r#"[{"b":0}]"#,
        &["-c", "try path(map(select(.b == 0))) catch ."],
    ),
    c(
        "[1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17]",
        &["-c", "try path(reverse) catch ."],
    ),
    c(
        "[1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17]",
        &[
            "-c",
            r#"try path(reverse | .["abcdefghijklmnopqrstuvwxyz"]) catch ."#,
        ],
    ),
    c(
        "[1,2]",
        &["-c", "[.[] as $x | try path(reverse | .[$x]) catch .]"],
    ),
    c("[1,2]", &["-c", "try path(reverse | .[]?) catch ."]),
    c("[1,2]", &["-c", "try path(reverse | .a?) catch ."]),
    c("[1,2]", &["-c", "try path(reverse | .[1:]) catch ."]),
    c("[1,2]", &["-c", "try path(reverse | ..) catch ."]),
    c("[1,2]", &["-c", "try path(reverse | map(.)) catch ."]),
    c("[1,2]", &["-c", "try path([1 | .a]) catch ."]),
    c(r#"{"a":1}"#, &["-c", "try path({a: 1} | .a) catch ."]),
    c(
        "[1,2]",
        &["-c", r#"try path(try error("x") catch .a) catch ."#],
    ),
    c("[1,2]", &["-c", "try path(-(1 | .a)) catch ."]),
    c(
        "null",
        &[
            "-c",
            r#"try path("aaaaaaaaaaaaaaaaaaaaaaaaéaaaaaaaaa") catch ."#,
        ],
    ),
    c(
        "null",
        &["-c", r#"try path(1 | .["aaaaaaaaéaaaaaa"]) catch ."#],
    ),
    c("null", &["-c", "[path(null), path(1 | null | .b)]"]),
    c(
        r#"{"a":null}"#,
        &["-c", "[path(.a | null), (try path(.a // null) catch .)]"],
    ),
    c("true", &["-c", "[path(true), (try path(false) catch .)]"]),
    c(
        r#"[{"a":0},{"a":1}]"#,
        &["-c", "try ((map(select(.a == 1))[].b) = 10) catch ."],
    ),
    c("[0,1,2]", &["-c", "try (def x: reverse; x = 10) catch ."]),
    c("[1,2]", &["-c", "try del(reverse) catch ."]),
    c(
        r#"{"a":[false,1,null,2]}"#,
        &["-c", "[path(.a[] // .b)], ((.a[] // .b) |= 7)"],
    ),
    c(
        r#"{"a":[false,1]}"#,
        &["-c", "try path([.a[] // 0]) catch ."],
    ),
    c("[1,2]", &["-c", "path(.[0] | tostring)"]),
    c("[1,2]", &["-c", "path(reverse | .[0])"]),
    c(r#"{"a":[1,2]}"#, &["-c", "del(.a | map(.) | .[0])"]),
    c("[1,2]\n[]\n[5]", &["-c", "path(reverse|.[0])"]),
    c("[1,2] [[3]]", &["-c", ".[0], path(.[0] | tostring)"]),
    // Malformed input, `fromjson` and bad JSON arguments: jq's parse errors
    // (`packaging/jaq/0013`); `%` with both sides clamped to integers (0012).
    c("{", &["-c", "."]),
    c("1 2 }", &["-c", "."]),
    c("{'a': 123}", &["-c", "."]),
    c(r#""\x""#, &["-c", "."]),
    c("[1,\n2,\n x", &["-c", "."]),
    c("1}", &["-c", "."]),
    c("truex 1", &["-c", "."]),
    c("[1,]", &["-c", "."]),
    c(r#"{"a" 1}"#, &["-c", "."]),
    c("{1:2}", &["-c", "."]),
    c("\"a\nb\"", &["-c", "."]),
    c("1 2 }", &["-nc", "[inputs]"]),
    c("1 2 }", &["-nc", "[try (input,input,input) catch .]"]),
    c("1 2 }", &["-sc", "."]),
    c(r#""{\"a\":}""#, &["-c", "try fromjson catch ."]),
    c(r#""1 2""#, &["-c", "try fromjson catch ."]),
    c(r#""""#, &["-c", "try fromjson catch ."]),
    c(r#""NaN1""#, &["-c", "try fromjson catch ."]),
    c("[1]", &["-c", "try fromjson catch ."]),
    c("null", &["-c", "--argjson", "x", "{", "$x"]),
    c("null", &["-c", "$ARGS", "--jsonargs", "1", "{"]),
    c(
        "null",
        &["-c", "[(infinite, -infinite) % (1, -1, infinite)]"],
    ),
    c(
        "null",
        &[
            "-c",
            "[5 % 2.5, -5 % 2.5, 1e19 % 7, 9223372036854775807 % 2, 9007199254740993 % 2, (nan % 0 | isnan)]",
        ],
    ),
    c("null", &["-c", "try (5 % 0.5) catch ."]),
    c("null", &["-c", "5 % 0.5"]),
    // Several files: the location names the file being read, its line count
    // starting again in each; a value ended by the next file's first byte is
    // reported in that file (`n1.json` then `c.json`). Fixtures in
    // `write_fixtures`.
    c("", &["-c", ".a", "l1.json", "l2.json"]),
    c("", &["-c", ".a", "n1.json", "c.json"]),
    c("", &["-c", ".a", "e.json", "l1.json", "e.json", "c.json"]),
    c("", &["-c", ".a", "h1.json", "h2.json"]),
    c("", &["-c", ".a", "g1.json", "g2.json", "e.json"]),
    c("", &["-c", ".a", "pre.json", "post.json"]),
    c("", &["-s", ".a", "l1.json", "l2.json"]),
    c(
        "",
        &["-n", "-c", r#"[inputs] | error("x")"#, "l1.json", "l2.json"],
    ),
    c("", &["-n", "-c", r#"error("x")"#, "l1.json", "l2.json"]),
    c("", &["-R", ".a", "r1.txt", "r2.txt"]),
    c(r#""in""#, &["-c", ".a", "l1.json", "-", "c.json"]),
    // An unreadable file is reported when the reader reaches it, one more
    // input is run, and jq exits with 2 whatever else happened.
    c("", &["-c", ".a", "l1.json", "missing.json", "l2.json"]),
    c("", &["-c", ".", "missing.json", "l1.json"]),
    c("", &["-c", ".a", "g1.json", "missing.json"]),
    c("", &["-s", "-c", ".", "l1.json", "missing.json", "l2.json"]),
    c("", &["-n", "-c", "[inputs]", "missing.json", "l1.json"]),
    c("", &["-n", "-c", "1", "missing.json", "l1.json"]),
    c("", &["-e", ".", "missing.json", "l1.json"]),
    c("", &["halt_error", "missing.json", "l1.json"]),
    c("", &["-c", ".", "l1.json", "dir", "l2.json"]),
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
    // Nothing comes between two files: a token, a string or a raw line runs
    // on into the next file.
    c("", &["-c", ".", "g1.json", "g2.json"]),
    c("", &["-c", ".", "s1.json", "s2.json"]),
    c("", &["-R", "-c", ".", "r1.txt", "r2.txt"]),
    c("", &["-R", "-s", "-c", ".", "r1.txt", "r2.txt"]),
];

/// Write the input files the cases name into `dir`.
fn write_fixtures(dir: &std::path::Path) {
    let files: &[(&str, &str)] = &[
        ("f1.json", r#"{"a":1}"#),
        ("f2.json", r#"{"a":2}"#),
        ("filt.jq", ".a\n"),
        ("filt2.jq", "[tostream]\n"),
        ("-5", "[5]"),
        ("l1.json", "1\n2\n\"x\"\n"),
        ("l2.json", "{\"a\":1}\n\n\"y\"\n3\n"),
        ("n1.json", "1 2"),
        ("c.json", "\"z\""),
        ("e.json", ""),
        ("h1.json", "[1,\n"),
        ("h2.json", "2]\n\"q\"\n"),
        ("g1.json", "12"),
        ("g2.json", "34\n\"w\"\n"),
        ("s1.json", "\"ab"),
        ("s2.json", "cd\"\n"),
        ("r1.txt", "line1\nline2"),
        ("r2.txt", "line3\nline4\n"),
    ];
    for (name, text) in files {
        std::fs::write(dir.join(name), text).expect("fixture");
    }
    // `fgets` restarts its 4095-byte chunks in each file: the array ends in
    // the first chunk of `post.json`, which holds no newline.
    let pre = format!("\"{}\"", "a".repeat(2000));
    let post = format!("[{}1]{}\n7\n", "1,".repeat(1249), " ".repeat(2000));
    std::fs::write(dir.join("pre.json"), pre).expect("fixture");
    std::fs::write(dir.join("post.json"), post).expect("fixture");
    std::fs::create_dir_all(dir.join("dir")).expect("fixture");
}

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

/// Whether the shim runs Pathfinder's patched jaq, found as the shim finds it.
fn engine_is_patched() -> bool {
    let jaq = std::env::var_os("PATHFINDER_JAQ")
        .or_else(|| option_env!("PATHFINDER_DEFAULT_JAQ").map(Into::into))
        .unwrap_or_else(|| "jaq".into());
    Command::new(jaq)
        .arg("--version")
        .output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains("+pathfinder"))
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
    let (code, stdout, _) = run_with(exe, cwd, case, Stdio::null());
    (code, stdout)
}

/// [`run`], also returning stderr when `stderr` is piped.
fn run_with(
    exe: &std::path::Path,
    cwd: &std::path::Path,
    case: &Case,
    stderr: Stdio,
) -> (Option<i32>, Vec<u8>, Vec<u8>) {
    use std::io::Write as _;
    let mut child = Command::new(exe)
        .args(case.args.iter().map(OsStr::new))
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(stderr)
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
    (out.status.code(), out.stdout, out.stderr)
}

/// On the patched engine, an uncaught error, `debug` and `halt_error` write
/// what jq writes to stderr, and the exit status is jq's.
#[test]
fn patched_engine_reports_errors_as_jq() {
    let Some(jq) = real_jq() else {
        skip_or_fail("no usable jq available");
        return;
    };
    if !engine_is_patched() {
        eprintln!("skipped: the engine is a stock jaq, which reports errors its own way");
        return;
    }
    let dir = std::env::temp_dir().join(format!("pathfinder-err-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    write_fixtures(&dir);
    let shim = shim(&dir);
    let mut failures: Vec<String> = Vec::new();
    for case in PATCHED_STDERR_CASES {
        let expected = run_with(&jq, &dir, case, Stdio::piped());
        let actual = run_with(&shim, &dir, case, Stdio::piped());
        if expected != actual {
            let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
            failures.push(format!(
                "  args {:?} stdin {:?}\n    jq   -> exit {:?} {:?} {:?}\n    shim -> exit {:?} {:?} {:?}",
                case.args,
                case.stdin,
                expected.0,
                text(&expected.1),
                text(&expected.2),
                actual.0,
                text(&actual.1),
                text(&actual.2),
            ));
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        failures.is_empty(),
        "{} of {} stderr cases diverged from jq:\n{}",
        failures.len(),
        PATCHED_STDERR_CASES.len(),
        failures.join("\n")
    );
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
    write_fixtures(&dir);
    let shim = shim(&dir);

    let patched: &[Case] = if engine_is_patched() {
        PATCHED_CASES
    } else {
        &[]
    };
    let mut failures: Vec<String> = Vec::new();
    for case in CASES.iter().chain(FILE_CASES).chain(patched) {
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

    let total = CASES.len() + FILE_CASES.len() + patched.len();
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

    // `"a" * 0` is `""` in jq 1.8.1 and `null` in a stock jaq. `*` of two
    // literals is left alone by the rewriter; Pathfinder's patched jaq
    // (`packaging/jaq/0005-string-repeat.patch`) repairs it, so there the
    // case must agree with jq instead. Documented in doc/DIVERGENCES.md.
    let case = c("null", &["-nc", r#""a"*0"#]);
    let expected = run(&jq, &dir, &case);
    let actual = run(&shim, &dir, &case);
    assert_eq!(String::from_utf8_lossy(&expected.1).trim(), r#""""#);
    let repaired = if engine_is_patched() { r#""""# } else { "null" };
    assert_eq!(String::from_utf8_lossy(&actual.1).trim(), repaired);

    // An object indexed with a number is jq's error, and `null` in a stock
    // jaq (`packaging/jaq/0006-object-keys.patch` repairs it).
    let case = c(r#"{"a":1}"#, &["-c", "try .[3] catch ."]);
    let expected = run(&jq, &dir, &case);
    let actual = run(&shim, &dir, &case);
    let _ = std::fs::remove_dir_all(&dir);
    let error = r#""Cannot index object with number""#;
    assert_eq!(String::from_utf8_lossy(&expected.1).trim(), error);
    let repaired = if engine_is_patched() { error } else { "null" };
    assert_eq!(String::from_utf8_lossy(&actual.1).trim(), repaired);
}
