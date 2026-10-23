//! JSON superset with binary data and non-string object keys.
//!
//! This crate provides a few macros for formatting / writing;
//! this is done in order to function with both
//! [`core::fmt::Write`] and [`std::io::Write`].
#![no_std]
#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![expect(
    clippy::clone_on_ref_ptr,
    clippy::default_trait_access,
    clippy::doc_markdown,
    clippy::flat_map_option,
    clippy::format_push_string,
    clippy::if_not_else,
    clippy::items_after_statements,
    clippy::manual_string_new,
    clippy::many_single_char_names,
    clippy::match_same_arms,
    clippy::naive_bytecount,
    clippy::needless_for_each,
    clippy::semicolon_if_nothing_returned,
    clippy::unnecessary_semicolon,
    reason = "jaq 3.1.1 as imported (crates/UPSTREAM.md): kept textually close to upstream so later upstream fixes port by diff (TODO T-020); restyling it would bury the patch set"
)]
#![expect(
    clippy::must_use_candidate,
    clippy::needless_pass_by_value,
    clippy::return_self_not_must_use,
    clippy::unnecessary_wraps,
    missing_debug_implementations,
    reason = "jaq 3.1.1 as imported (crates/UPSTREAM.md): changing the public signatures, derives or attributes of the engine's API ripples through every crate, and M19 reshapes that API"
)]
#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "jaq 3.1.1 as imported (crates/UPSTREAM.md): conversions between f64, isize and usize are how jaq models jq's numbers; they are changed one by one with the arithmetic, not wholesale at import"
)]
#![allow(
    clippy::enum_glob_use,
    clippy::renamed_function_params,
    reason = "jaq 3.1.1 as imported (crates/UPSTREAM.md): fires only in some build configurations (test builds, feature sets), where `expect` would go unfulfilled"
)]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

mod funs;
mod num;
// Pathfinder: jq's parse error messages.
mod parse_error;
#[macro_use]
pub mod write;
pub mod read;

use alloc::{borrow::ToOwned, boxed::Box, string::String, vec::Vec};
use bstr::{BStr, ByteSlice};
use bytes::{BufMut, Bytes, BytesMut};
use core::cmp::Ordering;
use core::fmt;
use core::hash::{Hash, Hasher};
use jaq_core::box_iter::box_once;
use jaq_core::{load, ops, path, val, Exn};

pub use funs::{bytes_valrs, funs};
pub use num::Num;

#[cfg(not(feature = "sync"))]
pub use alloc::rc::Rc;
#[cfg(feature = "sync")]
pub use alloc::sync::Arc as Rc;

#[cfg(feature = "serde")]
mod serde;

/// JSON superset with binary data and non-string object keys.
///
/// This is the default value type for jaq.
#[derive(Clone, Debug, Default)]
pub enum Val {
    #[default]
    /// Null
    Null,
    /// Boolean
    Bool(bool),
    /// Number
    Num(Num),
    /// Byte string
    BStr(Box<Bytes>),
    /// Text string (interpreted as UTF-8)
    ///
    /// Note that this does not require the actual bytes to be all valid UTF-8;
    /// this just means that the bytes are interpreted as UTF-8.
    /// An effort is made to preserve invalid UTF-8 as is, else
    /// replace invalid UTF-8 by the Unicode replacement character.
    TStr(Box<Bytes>),
    /// Array
    Arr(Rc<Vec<Val>>),
    /// Object
    Obj(Rc<Map<Val, Val>>),
}

#[cfg(feature = "sync")]
#[test]
fn val_send_sync() {
    fn send_sync<T: Send + Sync>(_: T) {}
    send_sync(Val::default())
}

#[cfg(target_arch = "x86_64")]
const _: () = {
    assert!(core::mem::size_of::<Val>() == 16);
};

/// Types and sets of types.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Type {
    // Pathfinder: `Int` is gone: indices and slice bounds take jq's numbers
    // (`Num::as_jq_index`, `Num::as_jq_bound`) and words.
    /*
    /// `"1" | sin` or `pow(2; "3")` or `fma(2; 3; "4")`
    Float,
    */
    // Pathfinder: `Num` (`-"a"`) is gone: negation words jq's own error.
    /// `{(0): 1}` or `0 | fromjson` or `0 | explode` or `"a b c" | split(0)`
    Str,
    /// `0 | sort` or `0 | implode` or `[] | .[0:] = 0`
    Arr,
    /// `0 | .[]` or `0 | .[0]` or `0 | keys` (array or object)
    Iter,
    /// `{}[0:1]` (string or array)
    Range,
}

impl Type {
    fn as_str(&self) -> &'static str {
        match self {
            //Self::Float => "floating-point number",
            Self::Str => "string",
            Self::Arr => "array",
            Self::Iter => "iterable (array or object)",
            Self::Range => "rangeable (array or string)",
        }
    }
}

/// Order-preserving map
pub type Map<K = Val, V = K> = indexmap::IndexMap<K, V, foldhash::fast::RandomState>;

/// Error that can occur during filter execution.
pub type Error = jaq_core::Error<Val>;
/// A value or an eRror.
pub type ValR = jaq_core::ValR<Val>;
/// A value or an eXception.
pub type ValX<'a> = jaq_core::ValX<'a, Val>;

// This is part of the Rust standard library since 1.76:
// <https://doc.rust-lang.org/std/rc/struct.Rc.html#method.unwrap_or_clone>.
// However, to keep MSRV low, we reimplement it here.
fn rc_unwrap_or_clone<T: Clone>(a: Rc<T>) -> T {
    Rc::try_unwrap(a).unwrap_or_else(|a| (*a).clone())
}

impl jaq_core::ValT for Val {
    fn jq_absent(&self, index: &Self) -> bool {
        match (self, index) {
            (Val::Null, _) => true,
            (Val::Obj(o), Val::BStr(_) | Val::TStr(_)) => !o.contains_key(index),
            (Val::Arr(a), Val::Num(n)) => {
                // NaN is neither: `map_index` raises jq's error for it.
                let (t, len) = (n.as_f64().trunc(), a.len() as f64);
                t >= len || -t > len
            }
            _ => false,
        }
    }

    fn jq_type(&self) -> &'static str {
        match self {
            Self::Null => "null",
            Self::Bool(_) => "boolean",
            Self::Num(_) => "number",
            Self::BStr(_) | Self::TStr(_) => "string",
            Self::Arr(_) => "array",
            Self::Obj(_) => "object",
        }
    }

    fn jq_string(&self) -> Option<String> {
        match self {
            Self::BStr(b) | Self::TStr(b) => Some(String::from_utf8_lossy(b).into_owned()),
            _ => None,
        }
    }

    fn from_num(n: &str) -> ValR {
        Ok(Self::Num(Num::from_str(n)))
    }

    fn from_map<I: IntoIterator<Item = (Self, Self)>>(iter: I) -> ValR {
        // Pathfinder: jq 1.8.1 builds objects only with string keys.
        let mut m = Map::default();
        for (k, v) in iter {
            if !matches!(k, Val::BStr(_) | Val::TStr(_)) {
                return Err(Error::str(format_args!(
                    "Cannot use {} as object key",
                    k.jq_quoted()
                )));
            }
            m.insert(k, v);
        }
        Ok(Self::obj(m))
    }

    fn key_values(self) -> Box<dyn Iterator<Item = Result<(Val, Val), Error>>> {
        let arr_idx = |(i, x)| Ok((Self::from(i as isize), x));
        match self {
            Self::Arr(a) => Box::new(rc_unwrap_or_clone(a).into_iter().enumerate().map(arr_idx)),
            Self::Obj(o) => Box::new(rc_unwrap_or_clone(o).into_iter().map(Ok)),
            _ => box_once(Err(Error::typ(self, Type::Iter.as_str()))),
        }
    }

    fn values(self) -> Box<dyn Iterator<Item = ValR>> {
        match self {
            Self::Arr(a) => Box::new(rc_unwrap_or_clone(a).into_iter().map(Ok)),
            Self::Obj(o) => Box::new(rc_unwrap_or_clone(o).into_iter().map(|(_k, v)| Ok(v))),
            _ => box_once(Err(Error::typ(self, Type::Iter.as_str()))),
        }
    }

    fn index(self, index: &Self) -> ValR {
        self.index_opt(index).map(|o| o.unwrap_or(Val::Null))
    }

    fn range(self, range: val::Range<&Self>) -> ValR {
        let fs = |b: Bytes, range, skip_take: SkipTakeFn| {
            Self::range_int(range)
                .map(|range_char| skip_take(range_char, &b))
                .map(|(skip, take)| b.slice(skip..skip + take))
        };
        match self {
            Val::BStr(b) => fs(*b, range, skip_take_bytes).map(Val::byte_str),
            Val::TStr(b) => fs(*b, range, skip_take_chars).map(Val::utf8_str),
            Val::Arr(a) => Self::range_int(range)
                .map(|range| skip_take(range, a.len()))
                .map(|(skip, take)| a.iter().skip(skip).take(take).cloned().collect()),
            // Pathfinder: jq slices `null` to `null`.
            Val::Null => Ok(Val::Null),
            _ => Err(Error::typ(self, Type::Range.as_str())),
        }
    }

    fn map_values<'a, I: Iterator<Item = ValX<'a>>>(
        self,
        opt: path::Opt,
        f: impl Fn(Self) -> I,
    ) -> ValX<'a> {
        match self {
            Self::Arr(a) => {
                let iter = rc_unwrap_or_clone(a).into_iter().flat_map(f);
                Ok(iter.collect::<Result<_, _>>()?)
            }
            Self::Obj(o) => {
                let iter = rc_unwrap_or_clone(o).into_iter();
                let iter = iter.filter_map(|(k, v)| f(v).next().map(|v| Ok((k, v?))));
                Ok(Self::obj(iter.collect::<Result<_, Exn<_>>>()?))
            }
            v => opt.fail(v, |v| Exn::from(Error::typ(v, Type::Iter.as_str()))),
        }
    }

    fn map_index<'a, I: Iterator<Item = ValX<'a>>>(
        mut self,
        index: &Self,
        opt: path::Opt,
        f: impl Fn(Self) -> I,
    ) -> ValX<'a> {
        // Pathfinder: `null` too, which jq updates as an empty array.
        if let (Val::Null | Val::BStr(_) | Val::TStr(_) | Val::Arr(_), Val::Obj(o)) = (&self, index)
        {
            let range = o.get(&Val::utf8_str("start"))..o.get(&Val::utf8_str("end"));
            return self.map_range(range, opt, f);
        };
        match self {
            // Pathfinder: jq 1.8.1 updates `null` as the container its key
            // implies, an object for a string and an array for a number
            // (`null | .a.b = 1` is `{"a":{"b":1}}`); deleting from it
            // leaves `null`.
            Val::Null if matches!(index, Val::BStr(_) | Val::TStr(_)) => Val::obj(Map::default())
                .map_index(index, opt, f)
                .map(|v| match v {
                    Val::Obj(o) if o.is_empty() => Val::Null,
                    v => v,
                }),
            Val::Null if matches!(index, Val::Num(_)) => Val::Arr(Rc::default())
                .map_index(index, opt, f)
                .map(|v| match v {
                    Val::Arr(a) if a.is_empty() => Val::Null,
                    v => v,
                }),
            // Pathfinder: jq 1.8.1 indexes an object only with a string.
            Val::Obj(_) if !matches!(index, Val::BStr(_) | Val::TStr(_)) => {
                opt.fail(self, |v| Exn::from(Error::index(v, index.clone())))
            }
            Val::Obj(ref mut o) => {
                use indexmap::map::Entry::{Occupied, Vacant};
                match Rc::make_mut(o).entry(index.clone()) {
                    Occupied(mut e) => {
                        let v = core::mem::take(e.get_mut());
                        match f(v).next().transpose()? {
                            Some(y) => e.insert(y),
                            // Pathfinder: the other keys keep their order, as
                            // in jq, where jaq swapped the last key in.
                            None => e.shift_remove(),
                        };
                    }
                    Vacant(e) => {
                        if let Some(y) = f(Val::Null).next().transpose()? {
                            e.insert(y);
                        }
                    }
                }
                Ok(self)
            }
            // Pathfinder: jq 1.8.1's array update. A fractional index is
            // truncated; an index past the end pads with `null`; deleting an
            // element that is not there changes nothing; jq's errors for an
            // index before the start, above 536870911 (`INT_MAX >> 2`) or NaN.
            Val::Arr(ref mut a) => {
                let Val::Num(n) = index else {
                    if let Val::Arr(_) = index {
                        let e = "Cannot update field at array index of array";
                        return opt.fail(self, |_| Exn::from(Error::str(e)));
                    }
                    return opt.fail(self, |v| Exn::from(Error::index(v, index.clone())));
                };
                let len = a.len();
                let t = n.as_f64().trunc();
                // The element's index, if it is in the array.
                let at = if t.is_nan() {
                    None
                } else if t < 0.0 {
                    (-t <= len as f64).then(|| len - (-t) as usize)
                } else {
                    (t < len as f64).then_some(t as usize)
                };
                let x = at.map_or(Val::Null, |i| core::mem::take(&mut Rc::make_mut(a)[i]));
                let Some(y) = f(x).next().transpose()? else {
                    if let Some(i) = at {
                        Rc::make_mut(a).remove(i);
                    }
                    return Ok(self);
                };
                let i = match at {
                    Some(i) => i,
                    None if t.is_nan() => {
                        let e = "Cannot set array element at NaN index";
                        return opt.fail(self, |_| Exn::from(Error::str(e)));
                    }
                    None if t < 0.0 => {
                        let e = "Out of bounds negative array index";
                        return opt.fail(self, |_| Exn::from(Error::str(e)));
                    }
                    None if t > 536_870_911.0 => {
                        return opt.fail(self, |_| Exn::from(Error::str("Array index too large")));
                    }
                    None => t as usize,
                };
                let a = Rc::make_mut(a);
                if i >= a.len() {
                    a.resize(i + 1, Val::Null);
                }
                a[i] = y;
                Ok(self)
            }
            // Pathfinder: jq's error for indexing a scalar.
            _ => opt.fail(self, |v| Exn::from(Error::index(v, index.clone()))),
        }
    }

    fn map_range<'a, I: Iterator<Item = ValX<'a>>>(
        mut self,
        range: val::Range<&Self>,
        opt: path::Opt,
        f: impl Fn(Self) -> I,
    ) -> ValX<'a> {
        // Pathfinder: jq updates a slice of `null` from `null`: nothing
        // leaves it `null`, an array replaces it, anything else fails.
        if let Val::Null = self {
            return match f(Val::Null).next().transpose()? {
                None => Ok(Val::Null),
                Some(Val::Arr(a)) => Ok(Val::Arr(a)),
                Some(_) => opt.fail(Val::Null, |_| {
                    let e = "A slice of an array can only be assigned another array";
                    Exn::from(Error::str(e))
                }),
            };
        }
        let fs = |b: Bytes, range, skip_take: SkipTakeFn, from: ValBytesFn, into: BytesValFn| {
            let (skip, take) = match Self::range_int(range) {
                Ok(range) => skip_take(range, &b),
                Err(e) => return opt.fail(into(b), |_| Exn::from(e)),
            };
            let str = into(b.slice(skip..skip + take));
            let y = f(str).map(|y| from(y?).map_err(Exn::from)).next();
            let y = y.transpose()?.unwrap_or_default();
            let mut b = BytesMut::from(b);
            bytes_splice(&mut b, skip, take, &y);
            Ok(into(b.freeze()))
        };
        let stb = skip_take_bytes;
        match self {
            Val::Arr(ref mut a) => {
                let (skip, take) = match Self::range_int(range) {
                    Ok(range) => skip_take(range, a.len()),
                    Err(e) => return opt.fail(self, |_| Exn::from(e)),
                };
                let arr = a.iter().skip(skip).take(take).cloned().collect();
                // Pathfinder: jq 1.8.1's message for a slice given no array.
                let not_arr = |_: Error| {
                    let e = "A slice of an array can only be assigned another array";
                    Exn::from(Error::str(e))
                };
                let y = f(arr).map(|y| y?.into_arr().map_err(not_arr)).next();
                let y = y.transpose()?.unwrap_or_default();
                Rc::make_mut(a).splice(skip..skip + take, (*y).clone());
                Ok(self)
            }
            Val::BStr(b) => fs(*b, range, stb, Val::into_byte_str, Val::byte_str),
            // Pathfinder: jq 1.8.1 cannot update a slice of a string.
            Val::TStr(_) => opt.fail(self, |_| {
                Exn::from(Error::str("Cannot update string slices"))
            }),
            // Pathfinder: jq's error for slicing what is no array.
            _ => {
                let obj = Val::obj(Map::default());
                opt.fail(self, |v| Exn::from(Error::index(v, obj)))
            }
        }
    }

    /// True if the value is neither null nor false.
    fn as_bool(&self) -> bool {
        !matches!(self, Self::Null | Self::Bool(false))
    }

    fn into_string(self) -> Self {
        match self {
            Self::BStr(b) | Self::TStr(b) => Self::TStr(b),
            _ => Self::utf8_str(self.to_json()),
        }
    }
}

impl jaq_std::ValT for Val {
    fn into_seq<S: FromIterator<Self>>(self) -> Result<S, Self> {
        match self {
            Self::Arr(a) => match Rc::try_unwrap(a) {
                Ok(a) => Ok(a.into_iter().collect()),
                Err(a) => Ok(a.iter().cloned().collect()),
            },
            _ => Err(self),
        }
    }

    fn is_int(&self) -> bool {
        self.as_num().is_some_and(Num::is_int)
    }

    fn as_isize(&self) -> Option<isize> {
        self.as_num().and_then(Num::as_isize)
    }

    fn as_f64(&self) -> Option<f64> {
        self.as_num().map(Num::as_f64)
    }

    fn is_utf8_str(&self) -> bool {
        matches!(self, Self::TStr(_))
    }

    fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::BStr(b) | Self::TStr(b) => Some(b),
            _ => None,
        }
    }

    fn as_sub_str(&self, sub: &[u8]) -> Self {
        match self {
            Self::BStr(b) => Self::byte_str(b.slice_ref(sub)),
            Self::TStr(b) => Self::utf8_str(b.slice_ref(sub)),
            _ => panic!(),
        }
    }

    fn from_utf8_bytes(b: impl AsRef<[u8]> + Send + 'static) -> Self {
        Self::utf8_str(Bytes::from_owner(b))
    }
}

/// Definitions of the standard library.
pub fn defs() -> impl Iterator<Item = load::parse::Def<&'static str>> {
    load::parse(include_str!("defs.jq"), |p| p.defs())
        .unwrap()
        .into_iter()
}

type ValBytesFn = fn(Val) -> Result<Bytes, Error>;
type BytesValFn = fn(Bytes) -> Val;
type SkipTakeFn = fn(val::Range<num::PosUsize>, &[u8]) -> (usize, usize);

fn skip_take(range: val::Range<num::PosUsize>, len: usize) -> (usize, usize) {
    let from = abs_bound(range.start, len, 0);
    let upto = abs_bound(range.end, len, len);
    (from, upto.saturating_sub(from))
}

fn skip_take_bytes(range: val::Range<num::PosUsize>, b: &[u8]) -> (usize, usize) {
    skip_take(range, b.len())
}

fn skip_take_chars(range: val::Range<num::PosUsize>, b: &[u8]) -> (usize, usize) {
    let byte_index = |num::PosUsize(pos, c)| {
        let mut chars = b.char_indices().map(|(start, ..)| start);
        if pos {
            chars.nth(c).unwrap_or(b.len())
        } else {
            chars.nth_back(c - 1).unwrap_or(0)
        }
    };
    let from_byte = range.start.map_or(0, byte_index);
    let upto_byte = range.end.map_or(b.len(), byte_index);
    (from_byte, upto_byte.saturating_sub(from_byte))
}

fn bytes_splice(b: &mut BytesMut, skip: usize, take: usize, replace: &[u8]) {
    let final_len = b.len() - take + replace.len();
    let post_take = skip + take..b.len();

    if replace.len() > take {
        b.resize(final_len, 0);
    }
    b.copy_within(post_take, skip + replace.len());
    b[skip..skip + replace.len()].copy_from_slice(replace);
    if replace.len() < take {
        b.truncate(final_len);
    }
}

/// If a range bound is given, absolutise and clip it between 0 and `len`,
/// else return `default`.
fn abs_bound(i: Option<num::PosUsize>, len: usize, default: usize) -> usize {
    i.map_or(default, |i| core::cmp::min(i.wrap(len).unwrap_or(0), len))
}

/// Absolutise an index and return result if it is inside [0, len).
fn abs_index(i: num::PosUsize, len: usize) -> Option<usize> {
    i.wrap(len).filter(|i| *i < len)
}

impl Val {
    /// Construct an object value.
    pub fn obj(m: Map) -> Self {
        Self::Obj(m.into())
    }

    /// Construct a string that is interpreted as UTF-8.
    pub fn utf8_str(s: impl Into<Bytes>) -> Self {
        Self::TStr(s.into().into())
    }

    /// Construct a string that is interpreted as bytes.
    pub fn byte_str(s: impl Into<Bytes>) -> Self {
        Self::BStr(s.into().into())
    }

    fn as_num(&self) -> Option<&Num> {
        match self {
            Self::Num(n) => Some(n),
            _ => None,
        }
    }

    fn into_byte_str(self) -> Result<Bytes, Error> {
        match self {
            Self::BStr(b) => Ok(*b),
            _ => Err(Error::typ(self, Type::Str.as_str())),
        }
    }

    /// If the value is an array, return it, else fail.
    fn into_arr(self) -> Result<Rc<Vec<Self>>, Error> {
        match self {
            Self::Arr(a) => Ok(a),
            _ => Err(Error::typ(self, Type::Arr.as_str())),
        }
    }

    /// Pathfinder: jq 1.8.1's slice bounds: `null` or NaN is none, a
    /// fractional bound is rounded outwards (`Num::as_jq_bound`), and any
    /// other value is jq's error.
    fn range_int(range: val::Range<&Self>) -> Result<val::Range<num::PosUsize>, Error> {
        let f = |i: Option<&Self>, end: bool| match i {
            None | Some(Val::Null) => Ok(None),
            Some(Val::Num(n)) => Ok(n.as_jq_bound(end)),
            Some(_) => Err(Error::str("Array/string slice indices must be integers")),
        };
        Ok(f(range.start, false)?..f(range.end, true)?)
    }

    fn to_json(&self) -> Vec<u8> {
        let mut buf = write::Buf(Vec::new());
        write::write_buf(&mut buf, &write::Pp::default(), 0, self).unwrap();
        buf.0
    }

    fn index_opt(self, index: &Self) -> Result<Option<Val>, Error> {
        Ok(match (self, index) {
            (Val::Null, _) => None,
            (Val::BStr(a), Val::Num(i @ (Num::Int(_) | Num::BigInt(_)))) => i
                .as_pos_usize()
                .and_then(|i| abs_index(i, a.len()))
                .map(|i| usize::from(a[i]).into()),
            (Val::Arr(a), Val::Num(i @ (Num::Int(_) | Num::BigInt(_)))) => i
                .as_pos_usize()
                .and_then(|i| abs_index(i, a.len()))
                .map(|i| a[i].clone()),
            // Pathfinder: a fractional index is truncated, and NaN, infinite
            // or too large indexes nothing, as in jq 1.8.1 (`-0` is 0).
            (Val::Arr(a), Val::Num(n @ (Num::Float(_) | Num::Dec(_)))) => n
                .as_jq_index()
                .and_then(|i| abs_index(i, a.len()))
                .map(|i| a[i].clone()),
            (Val::Arr(_), Val::Arr(y)) if y.is_empty() => Some(Val::Arr(Default::default())),
            (Val::Arr(x), Val::Arr(y)) => {
                // adapted from the implementation of the `indices` filter
                let iw = x.windows(y.len()).enumerate();
                let indices = iw.filter_map(|(i, w)| (w == **y).then_some(i));
                Some(indices.map(Val::from).collect())
            }
            (Val::Obj(o), i @ (Val::BStr(_) | Val::TStr(_))) => o.get(i).cloned(),
            // Pathfinder: jq 1.8.1 indexes an object only with a string.
            (s @ Val::Obj(_), _) => return Err(Error::index(s, index.clone())),
            (v @ (Val::BStr(_) | Val::TStr(_) | Val::Arr(_)), Val::Obj(o)) => {
                use jaq_core::ValT;
                let start = o.get(&Val::utf8_str("start"));
                let end = o.get(&Val::utf8_str("end"));
                // Pathfinder: jq slices with an object only when it has both.
                if start.is_none() || end.is_none() {
                    return Err(Error::str("Array/string slice indices must be integers"));
                }
                return v.range(start..end).map(Some);
            }
            (s, _) => return Err(Error::index(s, index.clone())),
        })
    }
}

impl From<bool> for Val {
    fn from(b: bool) -> Self {
        Self::Bool(b)
    }
}

impl From<isize> for Val {
    fn from(i: isize) -> Self {
        Self::Num(Num::Int(i))
    }
}

impl From<usize> for Val {
    fn from(i: usize) -> Self {
        Self::Num(Num::from_integral(i))
    }
}

impl From<f64> for Val {
    fn from(f: f64) -> Self {
        Self::Num(Num::Float(f))
    }
}

impl From<String> for Val {
    fn from(s: String) -> Self {
        Self::utf8_str(Bytes::from_owner(s))
    }
}

impl From<val::Range<Val>> for Val {
    fn from(r: val::Range<Val>) -> Self {
        let kv = |(k, v): (&str, Option<_>)| v.map(|v| (k.to_owned().into(), v));
        let kvs = [("start", r.start), ("end", r.end)];
        Val::obj(kvs.into_iter().flat_map(kv).collect())
    }
}

impl FromIterator<Self> for Val {
    fn from_iter<T: IntoIterator<Item = Self>>(iter: T) -> Self {
        Self::Arr(Rc::new(iter.into_iter().collect()))
    }
}

impl core::ops::Add for Val {
    type Output = ValR;
    fn add(self, rhs: Self) -> Self::Output {
        let concat_bytes = |l, r| {
            let mut buf = BytesMut::from(l);
            buf.put(r);
            buf
        };
        use Val::*;
        match (self, rhs) {
            // `null` is a neutral element for addition
            (Null, x) | (x, Null) => Ok(x),
            (Num(x), Num(y)) => Ok(Num(x + y)),
            (BStr(l), BStr(r)) => Ok(Val::byte_str(concat_bytes(*l, r))),
            (TStr(l), TStr(r)) => Ok(Val::utf8_str(concat_bytes(*l, r))),
            (Arr(mut l), Arr(r)) => {
                //std::dbg!(Rc::strong_count(&l));
                Rc::make_mut(&mut l).extend(r.iter().cloned());
                Ok(Arr(l))
            }
            (Obj(mut l), Obj(r)) => {
                Rc::make_mut(&mut l).extend(r.iter().map(|(k, v)| (k.clone(), v.clone())));
                Ok(Obj(l))
            }
            (l, r) => Err(Error::math(l, ops::Math::Add, r)),
        }
    }
}

impl core::ops::Sub for Val {
    type Output = ValR;
    fn sub(self, rhs: Self) -> Self::Output {
        match (self, rhs) {
            (Self::Num(x), Self::Num(y)) => Ok(Self::Num(x - y)),
            (Self::Arr(mut l), Self::Arr(r)) => {
                let r = r.iter().collect::<alloc::collections::BTreeSet<_>>();
                Rc::make_mut(&mut l).retain(|x| !r.contains(x));
                Ok(Self::Arr(l))
            }
            (l, r) => Err(Error::math(l, ops::Math::Sub, r)),
        }
    }
}

fn obj_merge(l: &mut Rc<Map>, r: Rc<Map>) {
    let l = Rc::make_mut(l);
    let r = rc_unwrap_or_clone(r).into_iter();
    r.for_each(|(k, v)| match (l.get_mut(&k), v) {
        (Some(Val::Obj(l)), Val::Obj(r)) => obj_merge(l, r),
        (Some(l), r) => *l = r,
        (None, r) => {
            l.insert(k, r);
        }
    });
}

impl core::ops::Mul for Val {
    type Output = ValR;
    fn mul(self, rhs: Self) -> Self::Output {
        use Val::*;
        match (self, rhs) {
            (Num(x), Num(y)) => Ok(Num(x * y)),
            // Pathfinder: jq 1.8.1's `binop_multiply` and `jv_string_repeat`.
            // The count is truncated (`"ab" * 1.5` is `"ab"`, `* 0.5` is `""`),
            // a negative or NaN count is `null`, and a result of 2 GiB or more
            // is an error rather than an allocation.
            (s @ (BStr(_) | TStr(_)), Num(n)) | (Num(n), s @ (BStr(_) | TStr(_))) => {
                let d = n.as_f64();
                if d < 0.0 || d.is_nan() {
                    return Ok(Null);
                }
                let count = if d > f64::from(i32::MAX) {
                    i32::MAX as usize
                } else {
                    d as usize
                };
                let len = match &s {
                    BStr(b) | TStr(b) => b.len(),
                    _ => unreachable!(),
                };
                if (len as u64).saturating_mul(count as u64) >= (i32::MAX as u64 - 8) {
                    return Err(Error::str("Repeat string result too long"));
                }
                match s {
                    BStr(b) => Ok(Self::byte_str(b.repeat(count))),
                    TStr(b) => Ok(Self::utf8_str(b.repeat(count))),
                    _ => unreachable!(),
                }
            }
            (Obj(mut l), Obj(r)) => {
                obj_merge(&mut l, r);
                Ok(Obj(l))
            }
            (l, r) => Err(Error::math(l, ops::Math::Mul, r)),
        }
    }
}

/// Split a string by a given separator string.
fn split<'a>(s: &'a [u8], sep: &'a [u8]) -> Box<dyn Iterator<Item = &'a [u8]> + 'a> {
    if s.is_empty() {
        Box::new(core::iter::empty())
    } else if sep.is_empty() {
        // Rust's `split` function with an empty separator ("")
        // yields an empty string as first and last result
        // to prevent this, we are using `chars` instead
        Box::new(s.char_indices().map(|(start, end, _)| &s[start..end]))
    } else {
        Box::new(s.split_str(sep))
    }
}

impl core::ops::Div for Val {
    type Output = ValR;
    fn div(self, rhs: Self) -> Self::Output {
        let fs = |x: Bytes, y: Bytes, into: BytesValFn| {
            split(&x, &y).map(|s| into(x.slice_ref(s))).collect()
        };
        match (self, rhs) {
            // Pathfinder: a zero divisor is jq 1.8.1's error, not infinity.
            (Self::Num(x), Self::Num(y)) if y.as_f64() == 0.0 => {
                Err(Error::math(Self::Num(x), ops::Math::Div, Self::Num(y)))
            }
            (Self::Num(x), Self::Num(y)) => Ok(Self::Num(x / y)),
            (Self::TStr(x), Self::TStr(y)) => Ok(fs(*x, *y, Val::utf8_str)),
            (Self::BStr(x), Self::BStr(y)) => Ok(fs(*x, *y, Val::byte_str)),
            (l, r) => Err(Error::math(l, ops::Math::Div, r)),
        }
    }
}

impl core::ops::Rem for Val {
    type Output = ValR;
    fn rem(self, rhs: Self) -> Self::Output {
        match (self, rhs) {
            // Pathfinder: jq's remainder; a divisor that truncates to zero
            // is an error for any number, as in jq 1.8.1.
            (Self::Num(x), Self::Num(y)) => match x.rem_jq(&y) {
                Some(r) => Ok(Self::Num(r)),
                None => Err(Error::math(Self::Num(x), ops::Math::Rem, Self::Num(y))),
            },
            (l, r) => Err(Error::math(l, ops::Math::Rem, r)),
        }
    }
}

impl core::ops::Neg for Val {
    type Output = ValR;
    fn neg(self) -> Self::Output {
        match self {
            Self::Num(n) => Ok(Self::Num(-n)),
            // Pathfinder: jq 1.8.1's message.
            x => {
                use jaq_core::ValT;
                Err(Error::str(format_args!(
                    "{} cannot be negated",
                    x.jq_quoted()
                )))
            }
        }
    }
}

impl PartialOrd for Val {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Val {
    fn cmp(&self, other: &Self) -> Ordering {
        use Ordering::{Equal, Greater, Less};
        match (self, other) {
            (Self::Null, Self::Null) => Equal,
            (Self::Bool(x), Self::Bool(y)) => x.cmp(y),
            (Self::Num(x), Self::Num(y)) => x.cmp(y),
            (Self::BStr(x) | Self::TStr(x), Self::BStr(y) | Self::TStr(y)) => x.cmp(y),
            (Self::Arr(x), Self::Arr(y)) => x.cmp(y),
            (Self::Obj(x), Self::Obj(y)) => match (x.len(), y.len()) {
                (0, 0) => Equal,
                (0, _) => Less,
                (_, 0) => Greater,
                _ => {
                    let mut l: Vec<_> = x.iter().collect();
                    let mut r: Vec<_> = y.iter().collect();
                    l.sort_by_key(|(k, _v)| *k);
                    r.sort_by_key(|(k, _v)| *k);
                    // TODO: make this nicer
                    let kl = l.iter().map(|(k, _v)| k);
                    let kr = r.iter().map(|(k, _v)| k);
                    let vl = l.iter().map(|(_k, v)| v);
                    let vr = r.iter().map(|(_k, v)| v);
                    kl.cmp(kr).then_with(|| vl.cmp(vr))
                }
            },

            // nulls are smaller than anything else
            (Self::Null, _) => Less,
            (_, Self::Null) => Greater,
            // bools are smaller than anything else, except for nulls
            (Self::Bool(_), _) => Less,
            (_, Self::Bool(_)) => Greater,
            // numbers are smaller than anything else, except for nulls and bools
            (Self::Num(_), _) => Less,
            (_, Self::Num(_)) => Greater,
            // etc.
            (Self::BStr(_) | Self::TStr(_), _) => Less,
            (_, Self::BStr(_) | Self::TStr(_)) => Greater,
            (Self::Arr(_), _) => Less,
            (_, Self::Arr(_)) => Greater,
        }
    }
}

impl PartialEq for Val {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Null, Self::Null) => true,
            (Self::Bool(x), Self::Bool(y)) => x == y,
            (Self::Num(x), Self::Num(y)) => x == y,
            (Self::BStr(x) | Self::TStr(x), Self::BStr(y) | Self::TStr(y)) => x == y,
            (Self::Arr(x), Self::Arr(y)) => x == y,
            (Self::Obj(x), Self::Obj(y)) => x == y,
            _ => false,
        }
    }
}

impl Eq for Val {}

impl Hash for Val {
    fn hash<H: Hasher>(&self, state: &mut H) {
        fn hash_with(u: u8, x: impl Hash, state: &mut impl Hasher) {
            state.write_u8(u);
            x.hash(state)
        }
        match self {
            Self::Num(n) => n.hash(state),
            // Num::hash() starts its hash with a 0 or 1, so we start with 2 here
            Self::Null => state.write_u8(2),
            Self::Bool(b) => state.write_u8(if *b { 3 } else { 4 }),
            Self::BStr(b) | Self::TStr(b) => hash_with(5, b, state),
            Self::Arr(a) => hash_with(6, a, state),
            Self::Obj(o) => {
                state.write_u8(7);
                // this is similar to what happens in `Val::cmp`
                let mut kvs: Vec<_> = o.iter().collect();
                kvs.sort_by_key(|(k, _v)| *k);
                kvs.iter().for_each(|(k, v)| (k, v).hash(state));
            }
        }
    }
}

/// Display bytes as valid UTF-8 string.
///
/// This maps invalid UTF-8 to the Unicode replacement character.
pub fn bstr(s: &(impl core::convert::AsRef<[u8]> + ?Sized)) -> impl fmt::Display + '_ {
    BStr::new(s)
}

impl fmt::Display for Val {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write::format(f, &write::Pp::default(), 0, self)
    }
}
