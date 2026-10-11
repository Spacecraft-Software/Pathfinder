use super::tabular;
use alloc::{boxed::Box, vec::Vec};
use core::fmt;
use jaq_core::native::{bome, v, Filter};
use jaq_core::{DataT, RunPtr};
use jaq_json::{Error, Val};

/// Convert a value fallibly and write the result.
macro_rules! ser {
    ($cv: ident, $fmt: expr, $ty: ty, $f: ident) => {{
        let fail = |e| serialise_fail(&$cv.1, $fmt, e);
        bome(<$ty>::try_from(&$cv.1).map_err(fail).map(|v| {
            let mut buf = Vec::new();
            v.$f(&mut buf).unwrap();
            Val::utf8_str(buf)
        }))
    }};
}

/// Serialisation filters: jq's `@csv` and `@tsv`.
pub fn funs<D: for<'a> DataT<V<'a> = Val>>() -> Box<[Filter<RunPtr<D>>]> {
    Box::new([
        ("@csv", v(0), |cv| ser!(cv, "CSV", tabular::Row, write_csv)),
        ("@tsv", v(0), |cv| ser!(cv, "TSV", tabular::Row, write_tsv)),
    ])
}

fn serialise_fail(i: &impl fmt::Display, fmt: &str, e: impl fmt::Display) -> Error {
    Error::str(format_args!("cannot serialise {i} as {fmt}: {e}"))
}
