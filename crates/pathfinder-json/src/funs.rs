use crate::{read, Error, Val, ValR};
use alloc::{boxed::Box, string::ToString, vec::Vec};
use bstr::ByteSlice;
use bytes::{BufMut, Bytes, BytesMut};
use jaq_core::box_iter::BoxIter;
use jaq_core::native::{bome, run, unary, v, Filter, Fun};
use jaq_core::{DataT, RunPtr};
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
        ("tobytes", v(0), |cv| {
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
        ("has", v(1), |cv| {
            unary(cv, |v, k| v.index_opt(&k).map(|o| o.is_some().into()))
        }),
        ("indices", v(1), |cv| {
            let to_int = |i: usize| Val::from(i as isize);
            unary(cv, move |x, v| {
                x.indices(&v).map(|idxs| idxs.map(to_int).collect())
            })
        }),
        ("bsearch", v(1), |cv| {
            let to_idx = |r: Result<_, _>| r.map_or_else(|i| -1 - i as isize, |i| i as isize);
            unary(cv, move |a, x| {
                a.as_arr().map(|a| Val::from(to_idx(a.binary_search(&x))))
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
