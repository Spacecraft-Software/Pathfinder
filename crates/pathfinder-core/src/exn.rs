//! Exceptions and errors.

use crate::{compile::TermId, filter::Vars, RcList};
use alloc::{boxed::Box, string::String, string::ToString, vec::Vec};
use core::fmt::{self, Display};

/// Exception.
///
/// This is either an error, a runtime halt, or control flow data internal to jaq.
/// Users should only be able to observe the first two cases.
///
/// Use [`crate::unwrap_valr`] to convert a [`crate::ValX`] to an error.
#[derive(Clone, Debug)]
pub struct Exn<'a, V>(pub(crate) Inner<'a, V>);

#[derive(Clone, Debug)]
pub(crate) enum Inner<'a, V> {
    Err(Box<Error<V>>),
    /// Tail-recursive call.
    ///
    /// This is used internally to execute tail-recursive filters.
    /// If this can be observed by users, then this is a bug.
    TailCall(Box<(&'a TermId, Vars<V>, CallInput<V>)>),
    Break(usize),
    Halt(i32),
}

#[derive(Clone, Debug)]
pub(crate) enum CallInput<V> {
    Run(V),
    Paths((V, RcList<V>)),
}

impl<V> CallInput<V> {
    pub fn unwrap_run(self) -> V {
        match self {
            Self::Run(v) => v,
            _ => panic!(),
        }
    }

    pub fn unwrap_paths(self) -> (V, RcList<V>) {
        match self {
            Self::Paths(vp) => vp,
            _ => panic!(),
        }
    }
}

impl<V> Exn<'_, V> {
    /// If the exception is an error, yield it, else yield the exception.
    pub fn get_err(self) -> Result<Error<V>, Self> {
        match self.0 {
            Inner::Err(e) => Ok(*e),
            _ => Err(self),
        }
    }

    /// If the exception halts, yield the exit code, else yield the exception.
    pub fn get_halt(self) -> Result<i32, Self> {
        match self.0 {
            Inner::Halt(code) => Ok(code),
            _ => Err(self),
        }
    }

    /// Create an exception intended to halt filter execution.
    ///
    /// This is used by the `halt/1` filter.
    pub fn halt(exit_code: i32) -> Self {
        Self(Inner::Halt(exit_code))
    }
}

impl<V> Exn<'_, V> {
    /// Pathfinder: whether this is jaq's error for an update of a target
    /// that is no path expression ([`Error::path_expr`]).
    pub(crate) fn is_path_expr(&self) -> bool {
        matches!(&self.0, Inner::Err(e) if matches!(&e.0, Part::Str(parts)
            if matches!(parts.first(), Some(Part::Str("invalid path expression with input ")))))
    }
}

impl<V> From<Error<V>> for Exn<'_, V> {
    fn from(e: Error<V>) -> Self {
        Exn(Inner::Err(Box::new(e)))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Part<V, S = &'static str> {
    Val(V),
    Str(S),
}

/// Error that occurred during filter execution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error<V>(Part<V, Vec<Part<V>>>);

impl<V> Error<V> {
    /// Create a new error from a value.
    pub fn new(v: V) -> Self {
        Self(Part::Val(v))
    }

    /// Create a path expression error.
    pub fn path_expr(v: V) -> Self {
        Self(Part::Str(Vec::from([
            Part::Str("invalid path expression with input "),
            Part::Val(v),
        ])))
    }
}

/// Pathfinder: type, arithmetic and indexing errors in jq 1.8.1's words,
/// where jaq said `cannot use 1 as number`, `cannot calculate 1 + "a"` and
/// `cannot index 1 with "a"`.
impl<V: crate::ValT> Error<V> {
    /// Create a type error: `v` is not a `typ`. The types jq has one
    /// message for get it; any other keeps jaq's, as jq's message for those
    /// depends on the function.
    pub fn typ(v: V, typ: &'static str) -> Self {
        use Part::{Str, Val};
        let jq = match typ {
            "iterable (array or object)" => alloc::format!("Cannot iterate over {}", v.jq_quoted()),
            "number" => alloc::format!("{} number required", v.jq_quoted()),
            "rangeable (array or string)" => {
                alloc::format!("Cannot index {} with object", v.jq_type())
            }
            _ => {
                return [Str("cannot use "), Val(v), Str(" as "), Str(typ)]
                    .into_iter()
                    .collect()
            }
        };
        Self::new(V::from(jq))
    }

    /// Create a math error: `{l} and {r} cannot be added`, …, with jq's
    /// remark on a zero divisor (one that `%` truncates to zero included).
    pub fn math(l: V, op: crate::ops::Math, r: V) -> Self {
        use crate::ops::Math;
        let verb = match op {
            Math::Add => "cannot be added",
            Math::Sub => "cannot be subtracted",
            Math::Mul => "cannot be multiplied",
            Math::Div => "cannot be divided",
            Math::Rem => "cannot be divided (remainder)",
        };
        let numbers = l.jq_type() == "number" && r.jq_type() == "number";
        let zero = numbers
            && matches!(op, Math::Div | Math::Rem)
            && r > V::from(-1_isize)
            && r < V::from(1_isize);
        let why = if zero {
            " because the divisor is zero"
        } else {
            ""
        };
        let msg = alloc::format!("{} and {} {verb}{why}", l.jq_quoted(), r.jq_quoted());
        Self::new(V::from(msg))
    }

    /// Create an indexing error: `Cannot index number with string "a"`, the
    /// key's text shown when it is a string of fewer than 30 bytes.
    pub fn index(l: V, r: V) -> Self {
        let msg = match r.jq_string() {
            Some(k) if k.len() < 30 => {
                alloc::format!("Cannot index {} with string \"{k}\"", l.jq_type())
            }
            _ => alloc::format!("Cannot index {} with {}", l.jq_type(), r.jq_type()),
        };
        Self::new(V::from(msg))
    }
}

impl<V: From<String>> Error<V> {
    /// Build an error from something that can be converted to a string.
    pub fn str(s: impl ToString) -> Self {
        Self(Part::Val(V::from(s.to_string())))
    }
}

/// Pathfinder: jq 1.8.1's errors for a path expression that lost its path,
/// raised where jq raises them: at the first path operation applied to the
/// value afterwards, else at the end of `path(f)`.
impl<V: From<String> + Display> Error<V> {
    /// `Invalid path expression with result v`.
    pub fn path_result(v: &V) -> Self {
        let v = dump_trunc(v.to_string(), 30);
        Self::new(V::from(alloc::format!(
            "Invalid path expression with result {v}"
        )))
    }

    /// `Invalid path expression near attempt to access element k of v`,
    /// `k` being the key already written as JSON.
    pub fn path_index(v: &V, k: String) -> Self {
        let (k, v) = (dump_trunc(k, 15), dump_trunc(v.to_string(), 30));
        let msg =
            alloc::format!("Invalid path expression near attempt to access element {k} of {v}");
        Self::new(V::from(msg))
    }

    /// `Invalid path expression near attempt to iterate through v`.
    pub fn path_iter(v: &V) -> Self {
        let v = dump_trunc(v.to_string(), 30);
        let msg = alloc::format!("Invalid path expression near attempt to iterate through {v}");
        Self::new(V::from(msg))
    }
}

/// Pathfinder: jq's `jv_dump_string_trunc` into a buffer of `size` bytes:
/// text longer than `size - 1` bytes is cut to `size - 4` and ends in `...`,
/// never splitting a character.
fn dump_trunc(s: String, size: usize) -> String {
    if s.len() < size {
        return s;
    }
    let mut end = size - 4;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    alloc::format!("{}...", &s[..end])
}

impl<V> FromIterator<Part<V>> for Error<V> {
    fn from_iter<T: IntoIterator<Item = Part<V>>>(iter: T) -> Self {
        Self(Part::Str(iter.into_iter().collect()))
    }
}

impl<V: From<String> + Display> Error<V> {
    /// Convert the error into a value to be used by `catch` filters.
    pub fn into_val(self) -> V {
        if let Part::Val(v) = self.0 {
            v
        } else {
            V::from(self.to_string())
        }
    }
}

impl<V: Display> Display for Error<V> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match &self.0 {
            Part::Val(v) => v.fmt(f),
            Part::Str(parts) => parts.iter().try_for_each(|part| match part {
                Part::Val(v) => v.fmt(f),
                Part::Str(s) => s.fmt(f),
            }),
        }
    }
}
