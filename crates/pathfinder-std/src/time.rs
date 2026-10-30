//! Pathfinder: jq 1.8.1's time builtins (`f_gmtime`, `f_localtime`,
//! `f_mktime`, `f_strftime`, `f_strflocaltime`, `f_strptime` in
//! `src/builtin.c`), on the C library calls they make ([`crate::ctime`]).
//! jq's `todate`, `fromdate` and their ISO 8601 forms are its own
//! definitions on these (`defs.jq`).

use crate::ctime::{self, Tm};
use crate::{Error, ValR, ValT};
use alloc::string::String;
use alloc::vec::Vec;

/// A C string's bytes: up to the first NUL, where jq's `jv_string_value`
/// is read to by the C library.
fn c_str(b: &[u8]) -> &[u8] {
    b.iter().position(|&c| c == 0).map_or(b, |n| &b[..n])
}

/// `time_t secs = fsecs`: truncated, and where C's conversion has no
/// value, the one x86-64 gives (`cvttsd2si`).
#[expect(
    clippy::cast_possible_truncation,
    reason = "C's conversion, range-checked first"
)]
fn time_t(f: f64) -> i64 {
    // 2^63, the first double past `i64::MAX`.
    const LIMIT: f64 = 9_223_372_036_854_775_808.0;
    if f.is_nan() || !(-LIMIT..LIMIT).contains(&f) {
        i64::MIN
    } else {
        f as i64
    }
}

/// `tm2jv`, with jq's fraction of a second added to the seconds.
fn tm2jv<V: ValT>(tm: &Tm, frac: f64) -> V {
    let f = f64::from;
    [
        f(tm.year) + 1900.0,
        f(tm.mon),
        f(tm.mday),
        f(tm.hour),
        f(tm.min),
        f(tm.sec) + frac,
        f(tm.wday),
        f(tm.yday),
    ]
    .into_iter()
    .map(V::from)
    .collect()
}

/// `jv2tm`: `None` for an element that is not a number. The fields are
/// normalised as `timegm` (or, for `local`, `mktime`) normalises them.
fn jv2tm<V: ValT>(a: &[V], local: bool) -> Option<Tm> {
    let nums: Vec<Option<f64>> = a.iter().take(8).map(ValT::as_f64).collect();
    let mut tm = ctime::from_array(&nums)?;
    if local {
        tm.isdst = -1;
        ctime::mktime(&mut tm);
    } else {
        ctime::timegm(&mut tm);
    }
    Some(tm)
}

fn broken_down<V: ValT>(v: &V, convert: fn(i64) -> Option<Tm>, message: &'static str) -> ValR<V> {
    let fsecs = v.as_f64().ok_or_else(|| Error::str(message))?;
    let tm = convert(time_t(fsecs))
        .ok_or_else(|| Error::str("error converting number of seconds since epoch to datetime"))?;
    Ok(tm2jv(&tm, fsecs - fsecs.floor()))
}

/// `gmtime`.
pub fn gmtime<V: ValT>(v: &V) -> ValR<V> {
    broken_down(v, ctime::gmtime, "gmtime() requires numeric inputs")
}

/// `localtime`.
pub fn localtime<V: ValT>(v: &V) -> ValR<V> {
    broken_down(v, ctime::localtime, "localtime() requires numeric inputs")
}

/// `mktime`: `timegm` of the broken-down time.
pub fn mktime<V: ValT>(v: V) -> ValR<V> {
    let a: Vec<V> = v
        .into_seq()
        .map_err(|_not_array| Error::str("mktime requires array inputs"))?;
    let mut tm =
        jv2tm(&a, false).ok_or_else(|| Error::str("mktime requires parsed datetime inputs"))?;
    match ctime::timegm(&mut tm) {
        -1 => Err(Error::str("invalid gmtime representation")),
        #[expect(clippy::cast_precision_loss, reason = "jq's number is a double")]
        t => Ok(V::from(t as f64)),
    }
}

/// `strftime` (`local` false) and `strflocaltime` (`local` true).
pub fn strftime<V: ValT>(v: V, fmt: &V, local: bool) -> ValR<V> {
    let name = if local { "strflocaltime" } else { "strftime" };
    let inputs = || Error::str(alloc::format!("{name}/1 requires parsed datetime inputs"));
    let v = if v.as_f64().is_some() {
        if local {
            localtime(&v)?
        } else {
            gmtime(&v)?
        }
    } else {
        v
    };
    let a: Vec<V> = v.into_seq().map_err(|_not_array| inputs())?;
    let fmt = fmt
        .as_utf8_bytes()
        .ok_or_else(|| Error::str(alloc::format!("{name}/1 requires a string format")))?;
    let tm = jv2tm(&a, local).ok_or_else(inputs)?;
    let fmt = c_str(fmt);
    match ctime::strftime(fmt, &tm, fmt.len() + 100) {
        Some(out) if !out.is_empty() || fmt.is_empty() => {
            Ok(V::from(String::from_utf8_lossy(&out).into_owned()))
        }
        _ => Err(Error::str(alloc::format!(
            "{name}/1: unknown system failure"
        ))),
    }
}

/// `strptime`: glibc's, with jq's sentinels for the fields it leaves.
pub fn strptime<V: ValT>(v: &V, fmt: &V) -> ValR<V> {
    let both = "strptime/1 requires string inputs and arguments";
    let (input, fmt) = match (v.as_utf8_bytes(), fmt.as_utf8_bytes()) {
        (Some(i), Some(f)) => (c_str(i), c_str(f)),
        _ => return Err(Error::str(both)),
    };
    let mut tm = Tm {
        wday: 8,
        yday: 367,
        ..Tm::default()
    };
    let end = ctime::strptime(input, fmt, &mut tm)
        .filter(|&end| end == input.len() || ctime::is_space(input[end]));
    let Some(end) = end else {
        return Err(Error::str(alloc::format!(
            "date \"{}\" does not match format \"{}\"",
            String::from_utf8_lossy(input),
            String::from_utf8_lossy(fmt)
        )));
    };
    let fields = tm.mday != 0 && (0..=11).contains(&tm.mon);
    if tm.wday == 8 && fields {
        ctime::set_tm_wday(&mut tm);
    }
    if tm.yday == 367 && fields {
        ctime::set_tm_yday(&mut tm);
    }
    let r: V = tm2jv(&tm, 0.0);
    if end == input.len() {
        return Ok(r);
    }
    let mut r: Vec<V> = r.into_seq().unwrap_or_default();
    let rest = String::from_utf8_lossy(&input[end..]).into_owned();
    r.push(V::from(rest));
    Ok(r.into_iter().collect())
}
