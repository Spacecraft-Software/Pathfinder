use super::tabular;
use jaq_core::native::{bome, v, Filter};
use jaq_core::{DataT, RunPtr};
use jaq_json::{Error, Val};

/// Convert a value fallibly and write the result.
macro_rules! ser {
    ($cv: ident, $fmt: expr, $ty: ty, $f: ident) => {{
        let fail = |e| serialise_fail($fmt, e);
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

/// Pathfinder: jq 1.8.1's messages: a row that is no array names the
/// format, a field that is no scalar says `csv row` for either format.
fn serialise_fail(fmt: &str, e: tabular::Error) -> Error {
    use jaq_core::ValT;
    match e {
        tabular::Error::Row(v) => {
            let fmt = fmt.to_ascii_lowercase();
            Error::str(format_args!(
                "{} cannot be {fmt}-formatted, only array",
                v.jq_quoted()
            ))
        }
        tabular::Error::Field(v) => {
            Error::str(format_args!("{} is not valid in a csv row", v.jq_quoted()))
        }
    }
}
