//! Tests for named core filters, sorted by name.
#![allow(
    clippy::semicolon_if_nothing_returned,
    reason = "jaq 3.1.1's tests as imported (crates/UPSTREAM.md); `allow`, not `expect`, because T-009 rewrites these tests and may remove the instances"
)]

pub mod common;

use common::give;
use serde_json::json;

yields!(bsearch_absent1, "[1, 3] | bsearch(0)", -1);
yields!(bsearch_absent2, "[1, 3] | bsearch(2)", -2);
yields!(bsearch_absent3, "[1, 3] | bsearch(4)", -3);
yields!(bsearch_present, "[1, 3] | [bsearch(1, 3)]", [0, 1]);

// Pathfinder: jq's `fromjson` rejects a string holding several values, and
// it prints an infinity as the largest double (patches 0002, 0004, 0013);
// measured from jq 1.8.1.
yields!(
    fromjson_inf,
    r#"[("Infinity", "+Infinity", "-Infinity") | fromjson | tostring]"#,
    [
        "1.7976931348623157e+308",
        "1.7976931348623157e+308",
        "-1.7976931348623157e+308"
    ]
);
yields!(fromjson_uint, r#"" 1" | fromjson"#, 1);
yields!(fromjson_pint, r#""+1" | fromjson"#, 1);
yields!(fromjson_nint, r#""-1" | fromjson"#, -1);

#[test]
fn has() {
    /* TODO: reenable these tests
    let err = Error::Index(Val::Null, Val::Int(0));
    fail(json!(null), "has(0)", err);
    let err = Error::Index(Val::Int(0), Val::Null);
    fail(json!(0), "has([][0])", err);
    let err = Error::Index(Val::Int(0), Val::Int(1));
    fail(json!(0), "has(1)", err);
    let err = Error::Index(Val::Str("a".to_string().into()), Val::Int(0));
    fail(json!("a"), "has(0)", err);
    */

    give(json!([0, null]), "has(0)", json!(true));
    give(json!([0, null]), "has(1)", json!(true));
    give(json!([0, null]), "has(2)", json!(false));

    give(json!({"a": 1, "b": null}), r#"has("a")"#, json!(true));
    give(json!({"a": 1, "b": null}), r#"has("b")"#, json!(true));
    give(json!({"a": 1, "b": null}), r#"has("c")"#, json!(false));
}

yields!(indices_str, r#""a,b, cd, efg" | indices(", ")"#, [3, 7]);
yields!(
    indices_arr_num,
    "[0, 1, 2, 1, 3, 1, 4] | indices(1)",
    [1, 3, 5]
);
yields!(
    indices_arr_arr,
    "[0, 1, 2, 3, 1, 4, 2, 5, 1, 2, 6, 7] | indices([1, 2])",
    [1, 8]
);
yields!(indices_arr_str, r#"["a", "b", "c"] | indices("b")"#, [1]);

yields!(indices_arr_empty, "[0, 1] | indices([])", json!([]));
yields!(indices_arr_larger, "[1, 2] | indices([1, 2, 3])", json!([]));

yields!(indices_arr_overlap, "[0, 0, 0] | indices([0, 0])", [0, 1]);
yields!(indices_str_overlap, r#""aaa" | indices("aa")"#, [0, 1]);
yields!(indices_str_gb1, r#""🇬🇧!" | indices("!")"#, [2]);
yields!(indices_str_gb2, r#""🇬🇧🇬🇧" | indices("🇬🇧")"#, [0, 2]);

yields!(length_str_foo, r#""ƒoo" | length"#, 3);
yields!(length_str_namaste, r#""नमस्ते" | length"#, 6);
yields!(length_obj, r#"{"a": 5, "b": 3} | length"#, 2);
yields!(length_int_pos, " 2 | length", 2);
yields!(length_int_neg, "-2 | length", 2);
yields!(length_float_pos, " 2.5 | length", 2.5);
yields!(length_float_neg, "-2.5 | length", 2.5);

yields!(tojson_fl0, "1.0 | tojson", "1.0");
yields!(tojson_fl1, "1.1 | tojson", "1.1");
// Pathfinder: jq writes NaN as `null` and an infinity as the largest double
// (patch 0002). jq rejects a division by zero, so the values come from `nan`
// and `infinite`; measured from jq 1.8.1.
yields!(tojson_nan, "nan | tojson", "null");
yields!(tojson_inf, "infinite | tojson", "1.7976931348623157e+308");
yields!(
    tojson_ninf,
    "-infinite | tojson",
    "-1.7976931348623157e+308"
);

#[test]
fn tonumber() {
    give(json!(1.0), "tonumber", json!(1.0));
    give(json!("1.0"), "tonumber", json!(1.0));
    give(json!("42"), "tonumber", json!(42));
    give(json!("null"), "try tonumber catch -7", json!(-7));
    give(json!("true"), "try tonumber catch -7", json!(-7));
    give(json!("str"), "try tonumber catch -7", json!(-7));
    give(json!("\"str\""), "try tonumber catch -7", json!(-7));
    give(json!("[3, 4]"), "try tonumber catch -7", json!(-7));
    give(json!("{\"a\": 1}"), "try tonumber catch -7", json!(-7));
}

#[test]
fn toboolean() {
    give(json!(false), "toboolean", json!(false));
    give(json!("true"), "toboolean", json!(true));
    give(json!("false"), "toboolean", json!(false));
    give(json!("null"), "try toboolean catch -7", json!(-7));
    give(json!("3"), "try toboolean catch -7", json!(-7));
    give(json!("str"), "try toboolean catch -7", json!(-7));
    give(json!("\"str\""), "try toboolean catch -7", json!(-7));
    give(json!("[3, 4]"), "try toboolean catch -7", json!(-7));
    give(json!("{\"a\": 1}"), "try toboolean catch -7", json!(-7));
}

#[test]
fn math_rem() {
    // Pathfinder: jq 1.8.1 truncates both operands to integers before taking
    // the remainder (patch 0012). Every value below was measured from jq 1.8.1
    // (the error wording since M20, as the engine now words it).
    give(json!(null), "-2 % -2", json!(0));
    give(json!(null), "-2 % -1", json!(0));
    give(
        json!(null),
        "try (-2 % 0) catch .",
        json!(
            "number (-2) and number (0) cannot be divided (remainder) because the divisor is zero"
        ),
    );
    give(json!(null), "-2 % 2.1", json!(0));
    give(json!(null), "-2 % 3", json!(-2));
    give(json!(null), "-2 % 2000000001", json!(-2));
    give(json!(null), "-1 % -2", json!(-1));
    give(json!(null), "-1 % -1", json!(0));
    give(
        json!(null),
        "try (-1 % 0) catch .",
        json!(
            "number (-1) and number (0) cannot be divided (remainder) because the divisor is zero"
        ),
    );
    give(json!(null), "-1 % 2.1", json!(-1));
    give(json!(null), "-1 % 3", json!(-1));
    give(json!(null), "-1 % 2000000001", json!(-1));
    give(json!(null), "0 % -2", json!(0));
    give(json!(null), "0 % -1", json!(0));
    give(
        json!(null),
        "try (0 % 0) catch .",
        json!(
            "number (0) and number (0) cannot be divided (remainder) because the divisor is zero"
        ),
    );
    give(json!(null), "0 % 2.1", json!(0));
    give(json!(null), "0 % 3", json!(0));
    give(json!(null), "0 % 2000000001", json!(0));
    give(json!(null), "2.1 % -2 | . * 1000 | round", json!(0));
    give(json!(null), "2.1 % -1 | . * 1000 | round", json!(0));
    give(
        json!(null),
        r#"try (2.1 % 0) catch "error""#,
        json!("error"),
    );
    give(json!(null), "2.1 % 2.1", json!(0));
    give(json!(null), "2.1 % 3", json!(2));
    give(json!(null), "2.1 % 2000000001", json!(2));
    give(json!(null), "3 % -2", json!(1));
    give(json!(null), "3 % -1", json!(0));
    give(
        json!(null),
        "try (3 % 0) catch .",
        json!(
            "number (3) and number (0) cannot be divided (remainder) because the divisor is zero"
        ),
    );
    give(json!(null), "3 % 2.1 | . * 1000 | round", json!(1000));
    give(json!(null), "3 % 3", json!(0));
    give(json!(null), "3 % 2000000001", json!(3));
    give(json!(null), "2000000001 % -2", json!(1));
    give(json!(null), "2000000001 % -1", json!(0));
    give(
        json!(null),
        "try (2000000001 % 0) catch .",
        json!("number (2000000001) and number (0) cannot be divided (remainder) because the divisor is zero"),
    );
    give(
        json!(null),
        "2000000001 % 2.1 | . * 1000 | round",
        json!(1000),
    );
    give(json!(null), "2000000001 % 3", json!(0));
    give(json!(null), "2000000001 % 2000000001", json!(0));
}

yields!(
    format_urid_invalid,
    r#"try ("%FF" | @urid) catch ."#,
    json!("string (\"%FF\") is not a valid uri encoding")
);
