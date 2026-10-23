use crate::{read, Error, Val, ValR};
use alloc::{boxed::Box, string::ToString, vec::Vec};
use bstr::ByteSlice;
use bytes::{BufMut, Bytes, BytesMut};
use jaq_core::box_iter::{box_once, BoxIter};
use jaq_core::native::{bome, run, unary, v, Filter, Fun};
use jaq_core::{DataT, Exn, RunPtr};
use jaq_std::ValT as _;

impl Val {
    /// Return 0 for null, the absolute value for numbers, and
    /// the length for strings, arrays, and objects.
    ///
    /// Fail on booleans.
    fn length(&self) -> ValR {
        match self {
            Val::Null => Ok(Val::from(0usize)),
            Val::Num(n) => Ok(Val::Num(n.length())),
            Val::TStr(s) => Ok(Val::from(s.chars().count())),
            Val::BStr(b) => Ok(Val::from(b.len())),
            Val::Arr(a) => Ok(Val::from(a.len())),
            Val::Obj(o) => Ok(Val::from(o.len())),
            // Pathfinder: jq's message.
            Val::Bool(_) => Err(Error::str(format_args!(
                "{} has no length",
                jaq_core::ValT::jq_quoted(self)
            ))),
        }
    }

    /// Return the indices of `y` in `self`.
    fn indices<'a>(&'a self, y: &'a Val) -> Result<Box<dyn Iterator<Item = usize> + 'a>, Error> {
        match (self, y) {
            (Val::BStr(_), Val::BStr(y)) | (Val::TStr(_), Val::TStr(y)) if y.is_empty() => {
                Ok(Box::new(core::iter::empty()))
            }
            (Val::TStr(x), Val::TStr(y)) => {
                let index = |(i, _, _)| x.get(i..i + y.len());
                let iw = x.char_indices().map_while(index).enumerate();
                Ok(Box::new(iw.filter_map(|(i, w)| (w == **y).then_some(i))))
            }
            (Val::BStr(x), Val::BStr(y)) => {
                let iw = x.windows(y.len()).enumerate();
                Ok(Box::new(iw.filter_map(|(i, w)| (w == **y).then_some(i))))
            }
            (Val::Arr(_), Val::Arr(y)) if y.is_empty() => Ok(Box::new(core::iter::empty())),
            (Val::Arr(x), Val::Arr(y)) => {
                let iw = x.windows(y.len()).enumerate();
                Ok(Box::new(iw.filter_map(|(i, w)| (w == **y).then_some(i))))
            }
            (Val::Arr(x), y) => {
                let ix = x.iter().enumerate();
                Ok(Box::new(ix.filter_map(move |(i, x)| (x == y).then_some(i))))
            }
            (x, y) => Err(Error::index(x.clone(), y.clone())),
        }
    }

    /// `a` contains `b` iff either
    /// * the string `b` is a substring of `a`,
    /// * every element in the array `b` is contained in some element of the array `a`,
    /// * for every key-value pair `k, v` in `b`,
    ///   there is a key-value pair `k, v'` in `a` such that `v'` contains `v`, or
    /// * `a` equals `b`.
    fn contains(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::BStr(l), Self::BStr(r)) | (Self::TStr(l), Self::TStr(r)) => l.contains_str(&**r),
            (Self::Arr(l), Self::Arr(r)) => r.iter().all(|r| l.iter().any(|l| l.contains(r))),
            (Self::Obj(l), Self::Obj(r)) => r
                .iter()
                .all(|(k, r)| l.get(k).is_some_and(|l| l.contains(r))),
            _ => self == other,
        }
    }

    fn to_bytes(&self) -> Result<Bytes, Self> {
        match self {
            Val::Num(n) => n
                .as_isize()
                .and_then(|i| u8::try_from(i).ok())
                .map(|u| Bytes::from(Vec::from([u])))
                .ok_or_else(|| self.clone()),
            Val::BStr(b) | Val::TStr(b) => Ok(*b.clone()),
            Val::Arr(a) => {
                let mut buf = BytesMut::new();
                for x in a.iter() {
                    buf.put(Val::to_bytes(x)?);
                }
                Ok(buf.into())
            }
            _ => Err(self.clone()),
        }
    }

    fn as_bytes_owned(&self) -> Option<Bytes> {
        match self {
            Self::BStr(b) | Self::TStr(b) => Some(*b.clone()),
            _ => None,
        }
    }

    fn as_utf8_bytes_owned(&self) -> Option<Bytes> {
        self.is_utf8_str().then(|| self.as_bytes_owned()).flatten()
    }

    /// Return bytes if the value is a (byte or text) string.
    pub fn try_as_bytes_owned(&self) -> Result<Bytes, Error> {
        self.as_bytes_owned()
            .ok_or_else(|| Error::typ(self.clone(), "string"))
    }

    /// Return bytes if the value is a text string.
    pub fn try_as_utf8_bytes_owned(&self) -> Result<Bytes, Error> {
        self.as_utf8_bytes_owned()
            .ok_or_else(|| Error::typ(self.clone(), "string"))
    }
}

/// Pathfinder: jq 1.8.1's `type_error`, `<type> (<value>) <message>`, the
/// value's JSON cut to 11 bytes and `...` when longer than 14, never
/// splitting a character (as `jaq/src/jq.rs` writes it for uncaught errors).
fn type_error(v: &Val, msg: &str) -> Error {
    let typ = match v {
        Val::Null => "null",
        Val::Bool(_) => "boolean",
        Val::Num(_) => "number",
        Val::BStr(_) | Val::TStr(_) => "string",
        Val::Arr(_) => "array",
        Val::Obj(_) => "object",
    };
    let s = v.to_string();
    let dump = if s.len() <= 14 {
        s
    } else {
        let end = s
            .char_indices()
            .map(|(i, c)| i + c.len_utf8())
            .take_while(|&end| end <= 11)
            .last()
            .unwrap_or(0);
        alloc::format!("{}...", &s[..end])
    };
    Error::str(format_args!("{typ} ({dump}) {msg}"))
}

self_cell::self_cell!(
    struct BytesValRs {
        owner: Bytes,

        #[not_covariant]
        dependent: ValRs,
    }
);

impl Iterator for BytesValRs {
    type Item = ValR;
    fn next(&mut self) -> Option<Self::Item> {
        self.with_dependent_mut(|_owner, iter| iter.next())
    }
}

type ValRs<'a> = BoxIter<'a, ValR>;

/// Apply a function to bytes and yield the resulting value results.
pub fn bytes_valrs(b: Bytes, f: impl FnOnce(&[u8]) -> ValRs) -> ValRs<'static> {
    Box::new(BytesValRs::new(b, |b| f(b)))
}

/// Functions of the standard library.
pub fn funs<D: for<'a> DataT<V<'a> = Val>>() -> impl Iterator<Item = Fun<D>> {
    base().into_vec().into_iter().map(run)
}

fn base<D: for<'a> DataT<V<'a> = Val>>() -> Box<[Filter<RunPtr<D>>]> {
    Box::new([
        ("_delpaths", v(1), |cv| unary(cv, delpaths)),
        // Pathfinder: `getpath($path) = $x` on the input it owns, so no
        // second reference makes each step copy it (`fromstream`, `reduce`).
        ("_setpath", v(2), |mut cv| {
            let x = cv.0.pop_var();
            let path = cv.0.pop_var();
            box_once(match path {
                Val::Arr(path) => setpath(cv.1, &path, &x),
                _ => Err(Exn::from(Error::str("Path must be specified as an array"))),
            })
        }),
        // Pathfinder: exactly one value, with jq 1.8.1's error messages.
        ("fromjson", v(0), |cv| {
            let parse = |s: &[u8]| read::parse_jq_single(s).map_err(Error::str);
            let s = match &cv.1 {
                Val::TStr(s) => Ok(&s[..]),
                v => Err(type_error(v, "only strings can be parsed")),
            };
            bome(s.and_then(parse))
        }),
        ("tojson", v(0), |cv| bome(Ok(Val::utf8_str(cv.1.to_json())))),
        ("_tobytes", v(0), |cv| {
            let fail = |v| Error::str(format_args!("cannot convert {v} to bytes"));
            bome(cv.1.to_bytes().map(Val::byte_str).map_err(fail))
        }),
        ("length", v(0), |cv| bome(cv.1.length())),
        // Pathfinder: jq 1.8.1 compares containment only between values of
        // one type, and words its error so.
        ("contains", v(1), |cv| {
            unary(cv, |x, y| {
                use jaq_core::ValT;
                if x.jq_type() != y.jq_type() {
                    let (x, y) = (x.jq_quoted(), y.jq_quoted());
                    return Err(Error::str(format_args!(
                        "{x} and {y} cannot have their containment checked"
                    )));
                }
                Ok(Val::from(x.contains(&y)))
            })
        }),
        // Pathfinder: the value as jq's error messages show it, for the
        // messages `defs.jq` raises.
        ("_jq_quoted", v(0), |cv| {
            bome(Ok(Val::from(jaq_core::ValT::jq_quoted(&cv.1))))
        }),
        ("has", v(1), |cv| unary(cv, has)),
        ("tonumber", v(0), |cv| bome(tonumber(cv.1))),
        ("toboolean", v(0), |cv| bome(toboolean(cv.1))),
        ("indices", v(1), |cv| {
            let to_int = |i: usize| Val::from(i as isize);
            unary(cv, move |x, v| {
                x.indices(&v).map(|idxs| idxs.map(to_int).collect())
            })
        }),
        ("bsearch", v(1), |cv| {
            let to_idx = |r: Result<_, _>| r.map_or_else(|i| -1 - i as isize, |i| i as isize);
            unary(cv, move |a, x| match a {
                Val::Arr(a) => Ok(Val::from(to_idx(a.binary_search(&x)))),
                // Pathfinder: jq 1.8.1's message.
                a => {
                    use jaq_core::ValT;
                    Err(Error::str(format_args!(
                        "{} cannot be searched from",
                        a.jq_quoted()
                    )))
                }
            })
        }),
    ])
}

/// Pathfinder: jq 1.8.1's `delpaths` (`jv_delpaths` in jq's `src/jv_aux.c`):
/// the paths are sorted and deleted together, so that deleting `.[0]` and
/// `.[2]` removes the elements the paths named in the input.
fn delpaths(v: Val, paths: Val) -> ValR {
    use jaq_core::ValT;
    let Val::Arr(paths) = paths else {
        return Err(Error::str("Paths must be specified as an array"));
    };
    let mut paths: Vec<Val> = paths.iter().cloned().collect();
    paths.sort();
    let mut lists = Vec::with_capacity(paths.len());
    for p in &paths {
        match p {
            Val::Arr(a) => lists.push(a.iter().cloned().collect::<Vec<_>>()),
            p => {
                let k = p.jq_type();
                return Err(Error::str(format_args!(
                    "Path must be specified as array, not {k}"
                )));
            }
        }
    }
    match lists.first() {
        None => Ok(v),
        Some(first) if first.is_empty() => Ok(Val::Null),
        Some(_) => delpaths_sorted(v, &lists, 0),
    }
}

/// jq's `delpaths_sorted`: the paths (sorted, each longer than `start`) are
/// grouped by their key at `start`; a group with a path ending there deletes
/// the key, any other recurses into the value at it.
fn delpaths_sorted(mut v: Val, paths: &[Vec<Val>], start: usize) -> ValR {
    use jaq_core::{path::Opt, ValT};
    let mut delkeys = Vec::new();
    let mut i = 0;
    while i < paths.len() {
        let key = &paths[i][start];
        let j = i + paths[i..].iter().take_while(|p| &p[start] == key).count();
        if paths[i].len() == start + 1 {
            delkeys.push(key.clone());
        } else {
            let sub = v.clone().index(key)?;
            if !matches!(sub, Val::Null) {
                let new = delpaths_sorted(sub, &paths[i..j], start + 1)?;
                let set = |_| core::iter::once(Ok(new.clone()));
                v = v
                    .map_index(key, Opt::Essential, set)
                    .map_err(|e| match e.get_err() {
                        Ok(e) => e,
                        Err(_) => Error::str("delpaths: unexpected exception"),
                    })?;
            }
        }
        i = j;
    }
    dels(v, &delkeys)
}

/// jq's `jv_dels`: delete the (sorted) keys from `v`.
fn dels(v: Val, keys: &[Val]) -> ValR {
    use jaq_core::ValT;
    if keys.is_empty() || matches!(v, Val::Null) {
        return Ok(v);
    }
    match v {
        Val::Arr(a) => {
            let len = a.len();
            let mut del = alloc::vec![false; len];
            for k in keys {
                match k {
                    Val::Num(n) => {
                        // C's `(int)` cast, as jq has it.
                        let f = n.as_f64().trunc();
                        let i = if f < 0.0 { len as f64 + f } else { f };
                        if i >= 0.0 && i < len as f64 {
                            del[i as usize] = true;
                        }
                    }
                    Val::Obj(o) => {
                        let start = o.get(&Val::utf8_str("start"));
                        let end = o.get(&Val::utf8_str("end"));
                        let r = Val::range_int(start..end)?;
                        let (skip, take) = crate::skip_take(r, len);
                        del[skip..skip + take].iter_mut().for_each(|d| *d = true);
                    }
                    k => {
                        let t = k.jq_type();
                        return Err(Error::str(format_args!(
                            "Cannot delete {t} element of array"
                        )));
                    }
                }
            }
            let kept = a
                .iter()
                .zip(del)
                .filter(|(_, d)| !d)
                .map(|(x, _)| x.clone());
            Ok(kept.collect())
        }
        Val::Obj(mut o) => {
            for k in keys {
                if !matches!(k, Val::BStr(_) | Val::TStr(_)) {
                    let t = k.jq_type();
                    return Err(Error::str(format_args!(
                        "Cannot delete {t} field of object"
                    )));
                }
            }
            let m = alloc::rc::Rc::make_mut(&mut o);
            for k in keys {
                m.shift_remove(k);
            }
            Ok(Val::Obj(o))
        }
        v => {
            let t = v.jq_type();
            Err(Error::str(format_args!("Cannot delete fields from {t}")))
        }
    }
}

/// Pathfinder: jq 1.8.1's `has`: an object takes a string key, an array a
/// number (truncated; NaN is no index), `null` has nothing.
/// `v | getpath(path) = x`: each step is the update `.[k] |= …` makes.
fn setpath<'a>(v: Val, path: &[Val], x: &Val) -> jaq_core::ValX<'a, Val> {
    match path.split_first() {
        None => Ok(x.clone()),
        Some((k, rest)) => jaq_core::ValT::map_index(v, k, jaq_core::path::Opt::Essential, |c| {
            core::iter::once(setpath(c, rest, x))
        }),
    }
}

fn has(v: Val, k: Val) -> ValR {
    use jaq_core::ValT;
    let fail = || {
        let (t, kt) = (v.jq_type(), k.jq_type());
        Err(Error::str(format_args!(
            "Cannot check whether {t} has a {kt} key"
        )))
    };
    match (&v, &k) {
        (Val::Null, _) => Ok(Val::Bool(false)),
        (Val::Obj(o), Val::BStr(_) | Val::TStr(_)) => Ok(Val::Bool(o.contains_key(&k))),
        (Val::Arr(a), Val::Num(n)) => {
            let f = n.as_f64();
            Ok(Val::Bool(
                !f.is_nan() && f.trunc() >= 0.0 && f.trunc() < a.len() as f64,
            ))
        }
        _ => fail(),
    }
}

/// Pathfinder: jq 1.8.1's `tonumber`: a number as it is; a string whole, as
/// C's `strtod` reads it (`nan`, `inf`, `+5`, `.5`, `1.` included), keeping
/// a literal's text as jq does; anything else is jq's error.
fn tonumber(v: Val) -> ValR {
    use jaq_core::ValT;
    let fail = |v: &Val| {
        Err(Error::str(format_args!(
            "{} cannot be parsed as a number",
            v.jq_quoted()
        )))
    };
    let s = match &v {
        Val::Num(_) => return Ok(v),
        Val::BStr(b) | Val::TStr(b) => match core::str::from_utf8(b) {
            Ok(s) => s,
            Err(_) => return fail(&v),
        },
        _ => return fail(&v),
    };
    let Ok(f) = s.parse::<f64>() else {
        return fail(&v);
    };
    let digits = s.trim_start_matches(['+', '-']);
    if !digits.starts_with(|c: char| c.is_ascii_digit() || c == '.') {
        // `nan`, `inf`, `infinity`, in any case.
        return Ok(Val::from(f));
    }
    // jq's literal: no sign `+`, no leading `.`, no trailing `.`.
    let lit = s.strip_prefix('+').unwrap_or(s);
    let lit = match lit.strip_prefix('-') {
        Some(rest) => alloc::format!("-{}", canonical_literal(rest)),
        None => canonical_literal(lit),
    };
    let n = crate::Num::from_str(&lit);
    // An integral negative zero stays one, as jq prints it (`-0`); any other
    // keeps its text (`-0.0`).
    if matches!(n, crate::Num::Int(0)) && lit.starts_with('-') {
        return Ok(Val::from(-0.0));
    }
    Ok(Val::Num(n))
}

/// A number literal's digits as JSON spells them: `0.5` for `.5`, `1` for
/// `1.`, leading zeros dropped (`01` is `1`).
fn canonical_literal(s: &str) -> alloc::string::String {
    let (mant, exp) = match s.find(['e', 'E']) {
        Some(i) => (&s[..i], &s[i..]),
        None => (s, ""),
    };
    let (int, frac) = match mant.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (mant, None),
    };
    let int = int.trim_start_matches('0');
    let int = if int.is_empty() { "0" } else { int };
    match frac {
        Some(f) if !f.is_empty() => alloc::format!("{int}.{f}{exp}"),
        _ => alloc::format!("{int}{exp}"),
    }
}

/// Pathfinder: jq 1.8.1's `toboolean`.
fn toboolean(v: Val) -> ValR {
    use jaq_core::ValT;
    match &v {
        Val::Bool(_) => Ok(v),
        Val::TStr(b) | Val::BStr(b) if &***b == b"true" => Ok(Val::Bool(true)),
        Val::TStr(b) | Val::BStr(b) if &***b == b"false" => Ok(Val::Bool(false)),
        _ => Err(Error::str(format_args!(
            "{} cannot be parsed as a boolean",
            v.jq_quoted()
        ))),
    }
}
