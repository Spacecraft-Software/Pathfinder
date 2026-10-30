//! Standard library for the jq language.
//!
//! The standard library provides a set of filters.
//! These filters are either implemented as definitions or as functions.
//! For example, the standard library provides the `map(f)` filter,
//! which is defined using the more elementary filter `[.[] | f]`.
//!
//! If you want to use the standard library in jaq, then
//! you'll likely only need [`funs`] and [`defs`].
//! Most other functions are relevant if you
//! want to implement your own native filters.
#![no_std]
#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![expect(
    clippy::semicolon_if_nothing_returned,
    clippy::unnecessary_semicolon,
    reason = "jaq 3.1.1 as imported (crates/UPSTREAM.md): kept textually close to upstream so later upstream fixes port by diff (TODO T-020); restyling it would bury the patch set"
)]
#![expect(
    clippy::copy_iterator,
    clippy::must_use_candidate,
    clippy::needless_pass_by_value,
    clippy::return_self_not_must_use,
    missing_debug_implementations,
    reason = "jaq 3.1.1 as imported (crates/UPSTREAM.md): changing the public signatures, derives or attributes of the engine's API ripples through every crate, and M19 reshapes that API"
)]
#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    reason = "jaq 3.1.1 as imported (crates/UPSTREAM.md): conversions between f64, isize and usize are how jaq models jq's numbers; they are changed one by one with the arithmetic, not wholesale at import"
)]
#![allow(
    clippy::allow_attributes_without_reason,
    clippy::cast_lossless,
    clippy::items_after_statements,
    clippy::redundant_else,
    clippy::unreadable_literal,
    dead_code,
    unused_imports,
    reason = "jaq 3.1.1 as imported (crates/UPSTREAM.md): fires only in some build configurations (test builds, feature sets), where `expect` would go unfulfilled"
)]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

#[cfg(feature = "time")]
mod ctime;
pub mod input;
#[cfg(feature = "math")]
mod math;
#[cfg(feature = "regex")]
mod regex;
#[cfg(feature = "time")]
mod time;

use alloc::string::{String, ToString};
use alloc::{boxed::Box, vec::Vec};
use bstr::{BStr, ByteSlice};
use jaq_core::box_iter::{box_once, BoxIter};
use jaq_core::native::{bome, run, unary, v, Filter, Fun};
use jaq_core::{load, Bind, Cv, DataT, Error, Exn, RunPtr, ValR, ValT as _, ValX, ValXs};

/// Definitions of the standard library.
pub fn defs() -> impl Iterator<Item = load::parse::Def<&'static str>> {
    load::parse(include_str!("defs.jq"), |p| p.defs())
        .unwrap()
        .into_iter()
}

/// Named filters available by default in jaq
/// which are implemented as native filters, such as `length`, `keys`, ...,
/// but also `now`, `debug`, `fromdateiso8601`, ...
///
/// This is the combination of [`base_funs`] and [`extra_funs`].
/// It does not include filters implemented by definition, such as `map`.
#[cfg(all(
    feature = "std",
    feature = "format",
    feature = "log",
    feature = "math",
    feature = "regex",
    feature = "time",
))]
pub fn funs<D: DataT>() -> impl Iterator<Item = Fun<D>>
where
    for<'a> D::V<'a>: ValT,
{
    base_funs().chain(extra_funs())
}

/// Minimal set of filters that are generic over the value type.
/// Return the minimal set of named filters available in jaq
/// which are implemented as native filters, such as `length`, `keys`, ...,
/// but not `now`, `debug`, `fromdateiso8601`, ...
///
/// Does not return filters from the standard library, such as `map`.
pub fn base_funs<D: DataT>() -> impl Iterator<Item = Fun<D>>
where
    for<'a> D::V<'a>: ValT,
{
    base_run().into_vec().into_iter().map(run)
}

/// Supplementary set of filters that are generic over the value type.
#[cfg(all(
    feature = "std",
    feature = "format",
    feature = "log",
    feature = "math",
    feature = "regex",
    feature = "time",
))]
pub fn extra_funs<D: DataT>() -> impl Iterator<Item = Fun<D>>
where
    for<'a> D::V<'a>: ValT,
{
    [std(), format(), math(), regex(), time(), log()]
        .into_iter()
        .flat_map(|fs| fs.into_vec().into_iter().map(run))
}

/// Values that the standard library can operate on.
pub trait ValT: jaq_core::ValT + Ord + From<f64> + From<usize> {
    /// Convert an array into a sequence.
    ///
    /// This returns the original value as `Err` if it is not an array.
    fn into_seq<S: FromIterator<Self>>(self) -> Result<S, Self>;

    /// True if the value is integer.
    fn is_int(&self) -> bool;

    /// Use the value as machine-sized integer.
    ///
    /// If this function returns `Some(_)`, then [`Self::is_int`] must return true.
    /// However, the other direction must not necessarily be the case, because
    /// there may be integer values that are not representable by `isize`.
    fn as_isize(&self) -> Option<isize>;

    /// Use the value as floating-point number.
    ///
    /// This succeeds for all numeric values,
    /// rounding too large/small ones to +/- Infinity.
    fn as_f64(&self) -> Option<f64>;

    /// True if the value is interpreted as UTF-8 string.
    fn is_utf8_str(&self) -> bool;

    /// If the value is a string (whatever its interpretation), return its bytes.
    fn as_bytes(&self) -> Option<&[u8]>;

    /// If the value is interpreted as UTF-8 string, return its bytes.
    fn as_utf8_bytes(&self) -> Option<&[u8]> {
        self.is_utf8_str().then(|| self.as_bytes()).flatten()
    }

    /// If the value is a string (whatever its interpretation), return its bytes, else fail.
    fn try_as_bytes(&self) -> Result<&[u8], Error<Self>> {
        self.as_bytes().ok_or_else(|| self.fail_str())
    }

    /// If the value is interpreted as UTF-8 string, return its bytes, else fail.
    fn try_as_utf8_bytes(&self) -> Result<&[u8], Error<Self>> {
        self.as_utf8_bytes().ok_or_else(|| self.fail_str())
    }

    /// If the value is a string and `sub` points to a slice of the string,
    /// shorten the string to `sub`, else panic.
    fn as_sub_str(&self, sub: &[u8]) -> Self;

    /// Interpret bytes as UTF-8 string value.
    fn from_utf8_bytes(b: impl AsRef<[u8]> + Send + 'static) -> Self;
}

/// Convenience trait for implementing the core functions.
trait ValTx: ValT + Sized {
    fn into_vec(self) -> Result<Vec<Self>, Error<Self>> {
        self.into_seq().map_err(|v| Error::typ(v, "array"))
    }

    fn try_as_isize(&self) -> Result<isize, Error<Self>> {
        self.as_isize()
            .ok_or_else(|| Error::typ(self.clone(), "integer"))
    }

    fn try_as_i32(&self) -> Result<i32, Error<Self>> {
        self.try_as_isize()?.try_into().map_err(Error::str)
    }

    fn try_as_f64(&self) -> Result<f64, Error<Self>> {
        self.as_f64()
            .ok_or_else(|| Error::typ(self.clone(), "number"))
    }

    /// Apply a function to an array.
    fn mutate_arr(self, f: impl FnOnce(&mut Vec<Self>)) -> ValR<Self> {
        let mut a = self.into_vec()?;
        f(&mut a);
        Ok(Self::from_iter(a))
    }

    /// Apply a function to an array.
    fn try_mutate_arr<'a, F>(self, f: F) -> ValX<'a, Self>
    where
        F: FnOnce(&mut Vec<Self>) -> Result<(), Exn<'a, Self>>,
    {
        let mut a = self.into_vec()?;
        f(&mut a)?;
        Ok(Self::from_iter(a))
    }

    fn round(self, f: impl FnOnce(f64) -> f64) -> ValR<Self> {
        Ok(if self.is_int() {
            self
        } else {
            let f = f(self.try_as_f64()?);
            if f.is_finite() {
                if isize::MIN as f64 <= f && f <= isize::MAX as f64 {
                    Self::from(f as isize)
                } else {
                    // print floating-point number without decimal places,
                    // i.e. like an integer
                    Self::from_num(&alloc::format!("{f:.0}"))?
                }
            } else {
                Self::from(f)
            }
        })
    }

    /// If the value is interpreted as UTF-8 string,
    /// return its `str` representation.
    fn try_as_str(&self) -> Result<&str, Error<Self>> {
        self.try_as_utf8_bytes()
            .and_then(|s| core::str::from_utf8(s).map_err(Error::str))
    }

    fn map_utf8_str<B>(self, f: impl FnOnce(&[u8]) -> B) -> ValR<Self>
    where
        B: AsRef<[u8]> + Send + 'static,
    {
        Ok(Self::from_utf8_bytes(f(self.try_as_utf8_bytes()?)))
    }

    fn trim_utf8_with(&self, f: impl FnOnce(&[u8]) -> &[u8]) -> ValR<Self> {
        Ok(self.as_sub_str(f(self.try_as_utf8_bytes()?)))
    }

    /// Helper function to strip away the prefix or suffix of a string.
    fn strip_fix<F>(self, fix: &Self, f: F) -> Result<Self, Error<Self>>
    where
        F: for<'a> FnOnce(&'a [u8], &[u8]) -> Option<&'a [u8]>,
    {
        Ok(match f(self.try_as_bytes()?, fix.try_as_bytes()?) {
            Some(sub) => self.as_sub_str(sub),
            None => self,
        })
    }

    fn fail_str(&self) -> Error<Self> {
        Error::typ(self.clone(), "string")
    }

    /// Pathfinder: a number as C's `(int)` cast makes it for jq 1.8.1's math
    /// functions taking an `int` (`ldexp`, `scalbln`, `jn`, `yn`): truncated,
    /// and `INT_MIN` for NaN or a value outside `int`, as on x86-64.
    fn try_as_c_int(&self) -> Result<i32, Error<Self>> {
        let f = self.try_as_f64()?;
        let t = f.trunc();
        Ok(
            if t.is_finite() && t >= f64::from(i32::MIN) && t <= f64::from(i32::MAX) {
                t as i32
            } else {
                i32::MIN
            },
        )
    }

    /// Pathfinder: an array for a function jq writes as a loop over it
    /// (`sort_by`, `group_by`, `min_by`, …), which fails as `.[]` does.
    fn into_vec_iter(self) -> Result<Vec<Self>, Error<Self>> {
        self.into_seq()
            .map_err(|v| Error::str(alloc::format!("Cannot iterate over {}", v.jq_quoted())))
    }

    /// Pathfinder: an array for `sort`, with jq's message otherwise.
    fn into_vec_sort(self) -> Result<Vec<Self>, Error<Self>> {
        self.into_seq().map_err(|v| {
            let q = v.jq_quoted();
            Error::str(alloc::format!(
                "{q} cannot be sorted, as it is not an array"
            ))
        })
    }

    /// Pathfinder: the bytes of a string, or `message` as the error.
    fn utf8_or(&self, message: &str) -> Result<&[u8], Error<Self>> {
        self.as_utf8_bytes()
            .ok_or_else(|| Error::str(String::from(message)))
    }
}
impl<T: ValT> ValTx for T {}

/// Sort array by the given function.
fn sort_by<'a, V: ValT>(xs: &mut [V], f: impl Fn(V) -> ValXs<'a, V>) -> Result<(), Exn<'a, V>> {
    // Some(e) iff an error has previously occurred
    let mut err = None;
    xs.sort_by_cached_key(|x| {
        if err.is_some() {
            return Vec::new();
        };
        match f(x.clone()).collect() {
            Ok(y) => y,
            Err(e) => {
                err = Some(e);
                Vec::new()
            }
        }
    });
    err.map_or(Ok(()), Err)
}

/// Group an array by the given function.
fn group_by<'a, V: ValT>(xs: Vec<V>, f: impl Fn(V) -> ValXs<'a, V>) -> ValX<'a, V> {
    let mut yx: Vec<(Vec<V>, V)> = xs
        .into_iter()
        .map(|x| Ok((f(x.clone()).collect::<Result<_, _>>()?, x)))
        .collect::<Result<_, Exn<_>>>()?;

    yx.sort_by(|(y1, _), (y2, _)| y1.cmp(y2));

    let mut grouped = Vec::new();
    let mut yx = yx.into_iter();
    if let Some((mut group_y, first_x)) = yx.next() {
        let mut group = Vec::from([first_x]);
        for (y, x) in yx {
            if group_y != y {
                grouped.push(V::from_iter(core::mem::take(&mut group)));
                group_y = y;
            }
            group.push(x);
        }
        if !group.is_empty() {
            grouped.push(V::from_iter(group));
        }
    }

    Ok(V::from_iter(grouped))
}

/// Get the minimum or maximum element from an array according to the given function.
fn cmp_by<'a, V: Clone, F, R>(xs: Vec<V>, f: F, replace: R) -> Result<Option<V>, Exn<'a, V>>
where
    F: Fn(V) -> ValXs<'a, V>,
    R: Fn(&[V], &[V]) -> bool,
{
    let iter = xs.into_iter();
    let mut iter = iter.map(|x| (x.clone(), f(x).collect::<Result<Vec<_>, _>>()));
    let (mut mx, mut my) = if let Some((x, y)) = iter.next() {
        (x, y?)
    } else {
        return Ok(None);
    };
    for (x, y) in iter {
        let y = y?;
        if replace(&my, &y) {
            (mx, my) = (x, y);
        }
    }
    Ok(Some(mx))
}

/// Convert a string into an array of its Unicode codepoints (with negative integers representing UTF-8 errors).
fn explode<V: ValT>(s: &[u8]) -> impl Iterator<Item = ValR<V>> + '_ {
    let invalid = [].iter();
    Explode { s, invalid }.map(|r| match r {
        Err(b) => Ok((-(b as isize)).into()),
        // conversion from u32 to isize may fail on 32-bit systems for high values of c
        Ok(c) => Ok(isize::try_from(c as u32).map_err(Error::str)?.into()),
    })
}

struct Explode<'a> {
    s: &'a [u8],
    invalid: core::slice::Iter<'a, u8>,
}
impl Iterator for Explode<'_> {
    type Item = Result<char, u8>;
    fn next(&mut self) -> Option<Self::Item> {
        self.invalid.next().map(|next| Err(*next)).or_else(|| {
            let (c, size) = bstr::decode_utf8(self.s);
            let (consumed, rest) = self.s.split_at(size);
            self.s = rest;
            c.map(Ok).or_else(|| {
                // invalid UTF-8 sequence, emit all invalid bytes
                self.invalid = consumed.iter();
                self.invalid.next().map(|next| Err(*next))
            })
        })
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let max = self.s.len();
        let min = self.s.len() / 4;
        let inv = self.invalid.as_slice().len();
        (min + inv, Some(max + inv))
    }
}

/// Pathfinder: the array a `…_by(f)` function works on, failing as jq's
/// does. jq writes them as `_impl(map([f]))`: on an object `map` succeeds and
/// the implementation fails with both (`object ({}) and array ([]) cannot be
/// sorted, as they are not both arrays`); anything else fails in `map`.
fn by_input<'a, V: ValT + 'a>(
    v: V,
    f: &dyn Fn(V) -> ValXs<'a, V>,
    verb: &str,
) -> Result<Vec<V>, Exn<'a, V>> {
    let v = match v.into_seq() {
        Ok(a) => return Ok(a),
        Err(v) => v,
    };
    if v.jq_type() != "object" {
        let q = v.jq_quoted();
        return Err(Exn::from(Error::str(alloc::format!(
            "Cannot iterate over {q}"
        ))));
    }
    let mut mapped = Vec::new();
    for x in v.clone().values() {
        let outputs: Vec<V> = f(x?).collect::<Result<_, _>>()?;
        mapped.push(V::from_iter(outputs));
    }
    let (q, m) = (v.jq_quoted(), V::from_iter(mapped).jq_quoted());
    let are = if verb == "sorted" {
        ", as they are not both arrays"
    } else {
        ""
    };
    Err(Exn::from(Error::str(alloc::format!(
        "{q} and {m} cannot be {verb}{are}"
    ))))
}

/// Pathfinder: jq's `min` (`max` when `max`) of an array, `null` when empty.
fn min_max<V: ValT>(v: V, max: bool) -> ValR<V> {
    let a: Vec<V> = v.into_seq().map_err(|v| {
        let q = v.jq_quoted();
        Error::str(alloc::format!("{q} and {q} cannot be iterated over"))
    })?;
    let mut best: Option<V> = None;
    for x in a {
        best = match best {
            Some(b) if (max && x < b) || (!max && x >= b) => Some(b),
            _ => Some(x),
        };
    }
    match best {
        Some(b) => Ok(b),
        // `null`, as the engine's `null` makes it.
        None => core::iter::empty::<V>()
            .collect::<V>()
            .index(&V::from(0usize)),
    }
}

/// Pathfinder: jq's `@sh`: the input, or each element of an input array,
/// quoted for a POSIX shell when a string and written as `tostring` writes it
/// otherwise; an array or object inside is jq's error.
fn sh<V: ValT>(v: V) -> ValR<V> {
    let items: Vec<V> = v.into_seq().unwrap_or_else(|v| alloc::vec![v]);
    let mut out = String::new();
    for (i, x) in items.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        if let Some(s) = x.as_utf8_bytes() {
            out.push('\'');
            out.push_str(&String::from_utf8_lossy(s).replace('\'', "'\\''"));
            out.push('\'');
        } else if matches!(x.jq_type(), "array" | "object") {
            let q = x.jq_quoted();
            return Err(Error::str(alloc::format!(
                "{q} can not be escaped for shell"
            )));
        } else {
            out.push_str(&x.to_string());
        }
    }
    Ok(V::from(out))
}

/// Pathfinder: `trim`, `ltrim` and `rtrim`, with jq's message for a
/// non-string.
fn trim<V: ValT>(v: &V, f: impl FnOnce(&[u8]) -> &[u8]) -> ValR<V> {
    Ok(v.as_sub_str(f(v.utf8_or("trim input must be a string")?)))
}

/// Pathfinder: jq's messages for `explode` and the functions written with it.
const EXPLODE: &str = "explode input must be a string";
const STARTS: &str = "startswith() requires string inputs";
const ENDS: &str = "endswith() requires string inputs";

/// Convert an array of Unicode codepoints into a string.
///
/// Pathfinder: as jq 1.8.1 does it: a fractional code point is truncated,
/// one that is no Unicode scalar value (negative, a surrogate, above
/// U+10FFFF, infinite) becomes U+FFFD, and a non-number or NaN is jq's
/// error. jaq read a negative number as a raw byte, which jq has no notion
/// of.
fn implode<V: ValT>(xs: &[V]) -> Result<Vec<u8>, Error<V>> {
    let mut v = Vec::with_capacity(xs.len());
    for x in xs {
        let Some(f) = x.as_f64().filter(|f| !f.is_nan()) else {
            let q = x.jq_quoted();
            return Err(Error::str(alloc::format!(
                "{q} can't be imploded, unicode codepoint needs to be numeric"
            )));
        };
        let c = u32::try_from(f.trunc() as i64)
            .ok()
            .and_then(char::from_u32);
        let c = c.unwrap_or(char::REPLACEMENT_CHARACTER);
        v.extend(c.encode_utf8(&mut [0; 4]).as_bytes())
    }
    Ok(v)
}

/// Pathfinder: `reverse` as jq 1.8.1 defines it,
/// `[.[length - 1 - range(0; length)]]`: an array reversed, and anything
/// else of length 0 (`null`, `""`, `{}`, `0`) an empty array. Any other value
/// fails as that definition does, on `length` or on the first `.[n]`.
fn reverse<V: ValT>(v: V) -> ValR<V> {
    let v = match v.into_seq::<Vec<V>>() {
        Ok(mut a) => {
            a.reverse();
            return Ok(V::from_iter(a));
        }
        Err(v) => v,
    };
    let empty = match v.jq_type() {
        "null" => true,
        "boolean" => {
            let q = v.jq_quoted();
            return Err(Error::str(alloc::format!("{q} has no length")));
        }
        "number" => v.as_f64().is_some_and(|f| f.abs() <= 0.0),
        "string" => v.as_bytes().is_some_and(<[u8]>::is_empty),
        // An object's length is its number of keys.
        _ => v.to_string() == "{}",
    };
    if empty {
        Ok(core::iter::empty().collect())
    } else {
        let t = v.jq_type();
        Err(Error::str(alloc::format!("Cannot index {t} with number")))
    }
}

fn once_or_empty<'a, T: 'a, E: 'a>(r: Result<Option<T>, E>) -> BoxIter<'a, Result<T, E>> {
    Box::new(r.transpose().into_iter())
}

#[allow(clippy::unit_arg)]
#[expect(
    clippy::too_many_lines,
    reason = "one entry per native function, as upstream lists them; Pathfinder's jq messages added lines"
)]
fn base_run<D: DataT>() -> Box<[Filter<RunPtr<D>>]>
where
    for<'a> D::V<'a>: ValT,
{
    let f = || [Bind::Fun(())].into();
    Box::new([
        // Pathfinder: native, not `def null: [][0];`, so that inside a path
        // expression `null` is a value, as jq 1.8.1's literal is, and not the
        // path operation `.[0]` on a constructed array. It is also cheaper.
        ("null", v(0), |_| {
            let empty: D::V<'_> = core::iter::empty().collect();
            bome(empty.index(&D::V::from(0usize)))
        }),
        ("floor", v(0), |cv| bome(cv.1.round(f64::floor))),
        ("round", v(0), |cv| bome(cv.1.round(f64::round))),
        ("ceil", v(0), |cv| bome(cv.1.round(f64::ceil))),
        // Pathfinder: jq 1.8.1's message.
        ("utf8bytelength", v(0), |cv| {
            let s = cv.1.as_utf8_bytes().ok_or_else(|| {
                let q = cv.1.jq_quoted();
                Error::str(alloc::format!("{q} only strings have UTF-8 byte length"))
            });
            bome(s.map(|s| (s.len() as isize).into()))
        }),
        // Pathfinder: jq's messages; `ascii_downcase` and `ascii_upcase` are
        // written with `explode` in jq, so they fail as it does.
        ("explode", v(0), |cv| {
            bome(cv.1.utf8_or(EXPLODE).and_then(|s| explode(s).collect()))
        }),
        ("implode", v(0), |cv| {
            let implode = |s: Vec<_>| implode(&s);
            let a =
                cv.1.into_seq()
                    .map_err(|_not_array| Error::str("implode input must be an array"));
            bome(a.and_then(implode).map(D::V::from_utf8_bytes))
        }),
        ("ascii_downcase", v(0), |cv| {
            let s = cv.1.utf8_or(EXPLODE).map(ByteSlice::to_ascii_lowercase);
            bome(s.map(D::V::from_utf8_bytes))
        }),
        ("ascii_upcase", v(0), |cv| {
            let s = cv.1.utf8_or(EXPLODE).map(ByteSlice::to_ascii_uppercase);
            bome(s.map(D::V::from_utf8_bytes))
        }),
        ("reverse", v(0), |cv| bome(reverse(cv.1))),
        ("sort", v(0), |cv| {
            bome(cv.1.into_vec_sort().map(|mut a| {
                a.sort();
                D::V::from_iter(a)
            }))
        }),
        ("sort_by", f(), |mut cv| {
            let (f, fc) = cv.0.pop_fun();
            let f = move |v| f.run((fc.clone(), v));
            // Pathfinder: jq's messages (`by_input`).
            box_once(by_input(cv.1, &f, "sorted").and_then(|mut a| {
                sort_by(&mut a, f)?;
                Ok(D::V::from_iter(a))
            }))
        }),
        ("group_by", f(), |mut cv| {
            let (f, fc) = cv.0.pop_fun();
            let f = move |v| f.run((fc.clone(), v));
            box_once(by_input(cv.1, &f, "sorted").and_then(|a| group_by(a, f)))
        }),
        ("_min_by_or_empty", f(), |mut cv| {
            let (f, fc) = cv.0.pop_fun();
            let f = move |v| f.run((fc.clone(), v));
            let a = by_input(cv.1, &f, "iterated over");
            once_or_empty(a.and_then(|a| cmp_by(a, f, |my, y| y < my)))
        }),
        ("_max_by_or_empty", f(), |mut cv| {
            let (f, fc) = cv.0.pop_fun();
            let f = move |v| f.run((fc.clone(), v));
            let a = by_input(cv.1, &f, "iterated over");
            once_or_empty(a.and_then(|a| cmp_by(a, f, |my, y| y >= my)))
        }),
        // Pathfinder: jq's messages. jq 1.8.1 writes `ltrimstr` with
        // `startswith` and `rtrimstr` with `endswith`, so they fail as those.
        ("startswith", v(1), |cv| {
            unary(cv, |v, s| {
                let (v, s) = (v.utf8_or(STARTS)?, s.utf8_or(STARTS)?);
                Ok(v.starts_with(s).into())
            })
        }),
        ("endswith", v(1), |cv| {
            unary(cv, |v, s| {
                let (v, s) = (v.utf8_or(ENDS)?, s.utf8_or(ENDS)?);
                Ok(v.ends_with(s).into())
            })
        }),
        ("ltrimstr", v(1), |cv| {
            unary(cv, |v, pre| {
                let (s, p) = (v.utf8_or(STARTS)?, pre.utf8_or(STARTS)?);
                Ok(s.strip_prefix(p)
                    .map_or_else(|| v.clone(), |sub| v.as_sub_str(sub)))
            })
        }),
        ("rtrimstr", v(1), |cv| {
            unary(cv, |v, suf| {
                let (s, x) = (v.utf8_or(ENDS)?, suf.utf8_or(ENDS)?);
                Ok(s.strip_suffix(x)
                    .map_or_else(|| v.clone(), |sub| v.as_sub_str(sub)))
            })
        }),
        // Pathfinder: jq 1.8.1's message for a non-string.
        ("trim", v(0), |cv| bome(trim(&cv.1, ByteSlice::trim))),
        ("ltrim", v(0), |cv| bome(trim(&cv.1, ByteSlice::trim_start))),
        ("rtrim", v(0), |cv| bome(trim(&cv.1, ByteSlice::trim_end))),
        // Pathfinder: `min` and `max` natively, with jq's message for a value
        // that is no array; of equal values, `min` keeps the first and `max`
        // the last, as jq does.
        ("nan", v(0), |_| bome(Ok(D::V::from(f64::NAN)))),
        ("infinite", v(0), |_| bome(Ok(D::V::from(f64::INFINITY)))),
        ("min", v(0), |cv| bome(min_max(cv.1, false))),
        ("max", v(0), |cv| bome(min_max(cv.1, true))),
        // Pathfinder: `@sh` natively, with jq's message for a value that
        // cannot be quoted.
        ("@sh", v(0), |cv| bome(sh(cv.1))),
        ("_escape_sh", v(0), |cv| {
            bome(
                cv.1.try_as_utf8_bytes()
                    .map(|s| ValT::from_utf8_bytes(s.replace(b"'", b"'\\''"))),
            )
        }),
        // Pathfinder: jq's `main` takes the status as C's `int ret =
        // jv_number_value(…)`: truncated, and the x86-64 conversion's
        // `INT_MIN` where it has no value (NaN, out of range).
        ("_halt", v(1), |mut cv| {
            let exit_code = cv.0.pop_var().try_as_f64().map(c_int).map_err(Exn::from);
            box_once(exit_code.and_then(|exit_code| Err(Exn::halt(exit_code))))
        }),
        // Pathfinder: `halt_error`'s type check, made before its message is
        // written (`f_halt_error`).
        ("_halt_check", v(1), |mut cv| {
            let code = cv.0.pop_var();
            bome(if code.as_f64().is_some() {
                Ok(cv.1)
            } else {
                Err(Error::str(alloc::format!(
                    "{} halt_error/1: number required",
                    cv.1.jq_quoted()
                )))
            })
        }),
    ])
}

/// C's conversion of a double to an `int`, as x86-64's `cvttsd2si` does
/// it: truncated, and `INT_MIN` where the result has no value.
#[expect(clippy::cast_possible_truncation, reason = "range-checked first")]
fn c_int(f: f64) -> i32 {
    if f.is_nan() || !(-2_147_483_649.0 < f && f < 2_147_483_648.0) {
        i32::MIN
    } else {
        f as i32
    }
}

#[cfg(feature = "std")]
fn now<V: From<String>>() -> Result<f64, Error<V>> {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|x| x.as_secs_f64())
        .map_err(Error::str)
}

#[cfg(feature = "std")]
fn std<D: DataT>() -> Box<[Filter<RunPtr<D>>]>
where
    for<'a> D::V<'a>: ValT,
{
    use std::env::vars;
    Box::new([
        ("env", v(0), |_| {
            bome(D::V::from_map(
                vars().map(|(k, v)| (D::V::from(k), D::V::from(v))),
            ))
        }),
        ("now", v(0), |_| bome(now().map(D::V::from))),
    ])
}

#[cfg(feature = "format")]
fn replace(s: &[u8], patterns: &[&str], replacements: &[&str]) -> Vec<u8> {
    let ac = aho_corasick::AhoCorasick::new(patterns).unwrap();
    ac.replace_all_bytes(s, replacements)
}

#[cfg(feature = "format")]
fn format<D: DataT>() -> Box<[Filter<RunPtr<D>>]>
where
    for<'a> D::V<'a>: ValT,
{
    const HTML_PATS: [&str; 5] = ["<", ">", "&", "\'", "\""];
    const HTML_REPS: [&str; 5] = ["&lt;", "&gt;", "&amp;", "&apos;", "&quot;"];
    Box::new([
        ("_escape_html", v(0), |cv| {
            bome(cv.1.map_utf8_str(|s| replace(s, &HTML_PATS, &HTML_REPS)))
        }),
        ("_unescape_html", v(0), |cv| {
            bome(cv.1.map_utf8_str(|s| replace(s, &HTML_REPS, &HTML_PATS)))
        }),
        ("_encode_uri", v(0), |cv| {
            bome(cv.1.map_utf8_str(|s| urlencoding::encode_binary(s).to_string()))
        }),
        // Pathfinder: jq's `@urid` (M20, P-067).
        ("_decode_uri", v(0), |cv| bome(jq_decode_uri(cv.1))),
        ("_encode_base64", v(0), |cv| {
            use base64::{engine::general_purpose::STANDARD, Engine};
            bome(cv.1.map_utf8_str(|s| STANDARD.encode(s)))
        }),
        // Pathfinder: jq's `@base64d` (M20, P-067).
        ("_decode_base64", v(0), |cv| bome(jq_decode_base64(cv.1))),
    ])
}

/// Pathfinder: jq's `@base64d`. jq decodes up to the first `=` and ignores
/// the rest, accepts missing padding and ignores the unused low bits of the
/// last character (`"QR"` is `"A"`); it rejects only a character outside
/// the standard alphabet before the `=`, or a single leftover character.
/// Bytes that are not UTF-8 are replaced as jq replaces them.
#[cfg(feature = "format")]
fn jq_decode_base64<V: ValT>(v: V) -> ValR<V> {
    let s = v.try_as_utf8_bytes()?;
    let text = s.split(|&c| c == b'=').next().unwrap_or_default();
    let sextet = |c: u8| match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    };
    let fail = |what| Err(Error::str(alloc::format!("{} {what}", v.jq_quoted())));
    let Some(sextets) = text.iter().map(|&c| sextet(c)).collect::<Option<Vec<u8>>>() else {
        return fail("is not valid base64 data");
    };
    if sextets.len() % 4 == 1 {
        return fail("trailing base64 byte found");
    }
    let mut bytes = Vec::with_capacity(sextets.len() / 4 * 3 + 2);
    for group in sextets.chunks(4) {
        let word = group
            .iter()
            .enumerate()
            .fold(0u32, |w, (i, &x)| w | u32::from(x) << (18 - 6 * i));
        // Two characters make one byte, three make two, four make three.
        let [_, b0, b1, b2] = word.to_be_bytes();
        bytes.extend_from_slice(&[b0, b1, b2][..group.len() - 1]);
    }
    Ok(V::from_utf8_bytes(jq_lossy_utf8(&bytes)))
}

/// Pathfinder: jq's `@urid`: a `%` must be followed by two hex digits, and
/// the decoded bytes must be UTF-8.
#[cfg(feature = "format")]
fn jq_decode_uri<V: ValT>(v: V) -> ValR<V> {
    let s = v.try_as_utf8_bytes()?;
    if !s.contains(&b'%') {
        return Ok(v);
    }
    let hex = |c: u8| char::from(c).to_digit(16);
    let mut bytes = Vec::with_capacity(s.len());
    let mut i = 0;
    let decoded = loop {
        match s.get(i..) {
            Some([b'%', high, low, ..]) => match (hex(*high), hex(*low)) {
                (Some(high), Some(low)) => {
                    bytes.push((high << 4 | low) as u8);
                    i += 3;
                }
                _ => break None,
            },
            Some([b'%', ..]) => break None,
            Some([c, ..]) => {
                bytes.push(*c);
                i += 1;
            }
            _ => break core::str::from_utf8(&bytes).is_ok().then_some(()),
        }
    };
    match decoded {
        Some(()) => Ok(V::from_utf8_bytes(bytes)),
        None => Err(Error::str(alloc::format!(
            "{} is not a valid uri encoding",
            v.jq_quoted()
        ))),
    }
}

/// Pathfinder: bytes as jq makes a string of them (`jvp_utf8_next`): each
/// sequence that is not UTF-8 becomes one U+FFFD. A stray continuation byte
/// or a byte no sequence starts with is one sequence; a sequence cut short
/// by a byte that does not continue it ends before that byte; one cut short
/// by the end of the input runs to the end; an overlong form, a surrogate or
/// a code point past U+10FFFF is one sequence of its full length.
#[cfg(feature = "format")]
fn jq_lossy_utf8(mut bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    while !bytes.is_empty() {
        let valid = match core::str::from_utf8(bytes) {
            Ok(_) => bytes.len(),
            Err(e) => e.valid_up_to(),
        };
        out.extend_from_slice(&bytes[..valid]);
        bytes = &bytes[valid..];
        let Some(&first) = bytes.first() else { break };
        let length = match first {
            0xC2..=0xDF => 2,
            0xE0..=0xEF => 3,
            0xF0..=0xF4 => 4,
            _ => 1,
        };
        let length = if length > bytes.len() {
            bytes.len()
        } else {
            (1..length)
                .find(|&i| bytes[i] & 0xC0 != 0x80)
                .unwrap_or(length)
        };
        out.extend_from_slice("\u{FFFD}".as_bytes());
        bytes = &bytes[length..];
    }
    out
}

#[cfg(feature = "math")]
fn math<D: DataT>() -> Box<[Filter<RunPtr<D>>]>
where
    for<'a> D::V<'a>: ValT,
{
    let rename = |name, (_name, arity, f): Filter<RunPtr<D>>| (name, arity, f);
    Box::new([
        math::f_f!(acos),
        math::f_f!(acosh),
        math::f_f!(asin),
        math::f_f!(asinh),
        math::f_f!(atan),
        math::f_f!(atanh),
        math::f_f!(cbrt),
        math::f_f!(cos),
        math::f_f!(cosh),
        math::f_f!(erf),
        math::f_f!(erfc),
        math::f_f!(exp),
        math::f_f!(exp10),
        math::f_f!(exp2),
        math::f_f!(expm1),
        math::f_f!(fabs),
        math::f_fi!(frexp),
        // Pathfinder: internal, as jq 1.8.1 has no `ilogb`.
        rename("_ilogb", math::f_i!(ilogb)),
        math::f_f!(j0),
        math::f_f!(j1),
        math::f_f!(lgamma),
        math::f_f!(log),
        math::f_f!(log10),
        math::f_f!(log1p),
        math::f_f!(log2),
        // logb is implemented in jaq-std
        math::f_ff!(modf),
        rename("nearbyint", math::f_f!(round)),
        // pow10 is implemented in jaq-std
        math::f_f!(rint),
        // significand is implemented in jaq-std
        math::f_f!(sin),
        math::f_f!(sinh),
        math::f_f!(sqrt),
        math::f_f!(tan),
        math::f_f!(tanh),
        math::f_f!(tgamma),
        math::f_f!(trunc),
        math::f_f!(y0),
        math::f_f!(y1),
        math::ff_f!(atan2),
        math::ff_f!(copysign),
        // drem is implemented in jaq-std
        math::ff_f!(fdim),
        math::ff_f!(fmax),
        math::ff_f!(fmin),
        math::ff_f!(fmod),
        math::ff_f!(hypot),
        math::if_f!(jn),
        math::fi_f!(ldexp),
        math::ff_f!(nextafter),
        // nexttoward is implemented in jaq-std
        math::ff_f!(pow),
        math::ff_f!(remainder),
        // scalb is implemented in jaq-std
        rename("scalbln", math::fi_f!(scalbn)),
        math::if_f!(yn),
        math::fff_f!(fma),
    ])
}

#[cfg(feature = "regex")]
/// Pathfinder: jq 1.8.1's `_match_impl(re; mode; test)` (`f_match`): the
/// array of matches, or with `test` whether there is one, built as jq
/// builds them; its errors are jq's.
fn match_impl<'a, D: DataT>(mut cv: Cv<'a, D>) -> ValR<D::V<'a>>
where
    D::V<'a>: ValT,
{
    let testmode = cv.0.pop_var();
    let modifiers = cv.0.pop_var();
    let re = cv.0.pop_var();
    let test = testmode == D::V::from(true);
    let input = cv.1.as_utf8_bytes().ok_or_else(|| {
        let q = cv.1.jq_quoted();
        Error::str(alloc::format!(
            "{q} cannot be matched, as it is not a string"
        ))
    })?;
    let not_str = |v: &D::V<'a>| Error::str(alloc::format!("{} is not a string", v.jq_quoted()));
    let re_bytes = re.as_utf8_bytes().ok_or_else(|| not_str(&re))?;
    let flags_str = match modifiers.jq_type() {
        "null" => "",
        "string" => core::str::from_utf8(
            modifiers
                .as_utf8_bytes()
                .ok_or_else(|| not_str(&modifiers))?,
        )
        .map_err(Error::str)?,
        _ => return Err(not_str(&modifiers)),
    };
    let flags = regex::Flags::new(flags_str)
        .map_err(|()| Error::str(alloc::format!("{flags_str} is not a valid modifier string")))?;
    let failure = |e: pathfinder_regex::Error| {
        Error::str(alloc::format!(
            "Regex failure: {}",
            alloc::string::String::from_utf8_lossy(&e.message())
        ))
    };
    let compiled = pathfinder_regex::Regex::new(re_bytes, flags.options).map_err(failure)?;
    let found = regex::matches(input, &compiled, &flags, test).map_err(failure)?;
    if test {
        return Ok(D::V::from(!found.is_empty()));
    }
    let key = |k: &str| D::V::from(k.to_string());
    let null = || {
        let empty: D::V<'a> = core::iter::empty().collect();
        empty.index(&D::V::from(0usize))
    };
    // A string of the input, as jq makes one: bytes that are not UTF-8
    // (a match starting inside a character) are replaced.
    let sub = |b: &[u8]| match core::str::from_utf8(b) {
        Ok(_) => cv.1.as_sub_str(b),
        Err(_) => D::V::from(alloc::string::String::from_utf8_lossy(b).into_owned()),
    };
    let num = |n: isize| D::V::from(n);
    let mut out = Vec::with_capacity(found.len());
    for m in found {
        let mut caps = Vec::with_capacity(m.captures.len());
        for c in m.captures {
            let string = match c.string {
                Some(b) => sub(b),
                None => null()?,
            };
            let name = match c.name {
                Some(n) => D::V::from(alloc::string::String::from_utf8_lossy(n).into_owned()),
                None => null()?,
            };
            let fields = if c.matched_text {
                [
                    (key("offset"), num(c.offset)),
                    (key("length"), num(c.length as isize)),
                    (key("string"), string),
                    (key("name"), name),
                ]
            } else {
                [
                    (key("offset"), num(c.offset)),
                    (key("string"), string),
                    (key("length"), num(c.length as isize)),
                    (key("name"), name),
                ]
            };
            caps.push(D::V::from_map(fields)?);
        }
        out.push(D::V::from_map([
            (key("offset"), num(m.offset as isize)),
            (key("length"), num(m.length as isize)),
            (key("string"), sub(m.string)),
            (key("captures"), caps.into_iter().collect()),
        ])?);
    }
    Ok(out.into_iter().collect())
}

#[cfg(feature = "regex")]
fn regex<D: DataT>() -> Box<[Filter<RunPtr<D>>]>
where
    for<'a> D::V<'a>: ValT,
{
    let vvv = || [Bind::Var(()), Bind::Var(()), Bind::Var(())].into();
    Box::new([("_match_impl", vvv(), |cv| bome(match_impl(cv)))])
}

#[cfg(feature = "time")]
fn time<D: DataT>() -> Box<[Filter<RunPtr<D>>]>
where
    for<'a> D::V<'a>: ValT,
{
    // Pathfinder: jq 1.8.1's C functions on glibc (`time.rs`); `todate`,
    // `fromdate` and the ISO 8601 forms are jq's definitions (`defs.jq`).
    Box::new([
        ("strftime", v(1), |cv| {
            unary(cv, |v, fmt| time::strftime(v, &fmt, false))
        }),
        ("strflocaltime", v(1), |cv| {
            unary(cv, |v, fmt| time::strftime(v, &fmt, true))
        }),
        ("gmtime", v(0), |cv| bome(time::gmtime(&cv.1))),
        ("localtime", v(0), |cv| bome(time::localtime(&cv.1))),
        ("strptime", v(1), |cv| {
            unary(cv, |v, fmt| time::strptime(&v, &fmt))
        }),
        ("mktime", v(0), |cv| bome(time::mktime(cv.1))),
    ])
}

#[cfg(feature = "log")]
fn log<D: DataT>() -> Box<[Filter<RunPtr<D>>]>
where
    for<'a> D::V<'a>: ValT,
{
    fn eprint_raw<V: ValT>(v: &V) {
        if let Some(s) = v.as_utf8_bytes() {
            log::error!("{}", BStr::new(s))
        } else {
            log::error!("{v}")
        }
    }
    /// Construct a filter that applies an effect function before returning nothing.
    macro_rules! empty_with {
        ( $eff:expr ) => {
            |cv| {
                $eff(&cv.1);
                Box::new(core::iter::empty())
            }
        };
    }
    Box::new([
        ("_debug_empty", v(0), empty_with!(|x| log::debug!("{x}"))),
        ("_stderr_empty", v(0), empty_with!(eprint_raw)),
    ])
}
