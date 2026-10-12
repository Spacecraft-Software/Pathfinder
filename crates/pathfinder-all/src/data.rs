//! Commonly used data for filter compilation & execution.
use jaq_core::{data, DataT, Lut, Vars};
use jaq_fmts::write::Writer;
use jaq_json::{Val, ValX};
use jaq_std::input::{self, Inputs, RcIter};

/// Filter for given kind of data.
pub type Filter = jaq_core::Filter<DataKind>;

/// Execution context for given kind of data.
pub type Ctx<'a> = jaq_core::Ctx<'a, DataKind>;

/// Kind of data.
pub struct DataKind;

impl DataT for DataKind {
    type V<'a> = Val;
    type Data<'a> = &'a Data<'a>;
}

/// The actual data.
pub struct Data<'a> {
    /// run options
    pub runner: &'a Runner,
    /// look-up table
    pub lut: &'a Lut<DataKind>,
    /// input values
    pub inputs: Inputs<'a, Val>,
    /// Pathfinder: the name of the file the reader has open, for
    /// `input_filename`; `None` before any input is read.
    pub filename: &'a dyn Fn() -> Option<String>,
    /// Pathfinder: the lines the reader has fetched, for
    /// `input_line_number`.
    pub line_number: &'a dyn Fn() -> usize,
}

impl<'a> data::HasLut<'a, DataKind> for &'a Data<'a> {
    fn lut(&self) -> &'a Lut<DataKind> {
        self.lut
    }
}

impl<'a> input::HasInputs<'a, Val> for &'a Data<'a> {
    fn inputs(&self) -> Inputs<'a, Val> {
        self.inputs
    }
}

/// Run options.
#[derive(Default)]
pub struct Runner {
    /// pass `null` as input
    pub null_input: bool,
    /// use colors in error messages
    pub color_err: bool,
    /// write options
    pub writer: Writer,
}

impl Runner {
    /// Use colors on standard output?
    pub fn color_stdout(&self) -> bool {
        !self.writer.pp.styles.reset.is_empty()
    }
}

/// Functions from [`jaq_std`] and [`jaq_json`].
#[cfg(feature = "std-all")]
pub fn base_funs() -> impl Iterator<Item = crate::Fun<DataKind>> {
    let run = jaq_core::native::run::<DataKind>;
    let core = jaq_core::funs::<DataKind>();
    let std = jaq_std::funs::<DataKind>();
    let input = input::funs::<DataKind>().into_vec().into_iter().map(run);
    core.chain(std).chain(jaq_json::funs()).chain(input)
}

/// Base functions ([`base_funs`]) plus functions from [`jaq_fmts`].
#[cfg(feature = "std-all")]
pub fn funs() -> impl Iterator<Item = crate::Fun<DataKind>> {
    base_funs()
        .chain(jaq_fmts::funs())
        .chain(core::iter::once(input_filename()))
        .chain(core::iter::once(input_line_number()))
}

/// Pathfinder: `input_line_number`, as jq 1.8.1 answers it: the lines its
/// reader has fetched from the current input, the count its error locations
/// show ([`Data::line_number`]).
#[cfg(feature = "std-all")]
fn input_line_number() -> crate::Fun<DataKind> {
    jaq_core::native::run(("input_line_number", jaq_core::native::v(0), |cv| {
        let n = (cv.0.data().line_number)();
        Box::new(core::iter::once(Ok(Val::from(
            isize::try_from(n).unwrap_or(isize::MAX),
        ))))
    }))
}

/// Pathfinder: `input_filename`, as jq 1.8.1 answers it: the file the reader
/// has open, which the parser may already have entered for the next value,
/// and `null` before any input is read ([`Data::filename`]).
#[cfg(feature = "std-all")]
fn input_filename() -> crate::Fun<DataKind> {
    jaq_core::native::run(("input_filename", jaq_core::native::v(0), |cv| {
        let name = (cv.0.data().filename)();
        Box::new(core::iter::once(Ok(name.map_or(Val::Null, Val::utf8_str))))
    }))
}

/// Compile a filter without access to external files/variables, including all functions/definitions.
#[cfg(feature = "std-all")]
pub fn compile(code: &str) -> Result<Filter, Vec<crate::FileReports>> {
    crate::compile_with(code, crate::defs(), funs(), &[])
}

/// Run a filter with given input values and run `f` for every value output.
///
/// This function cannot return an `Iterator` because it creates an `RcIter`.
/// This is most unfortunate. We should think about how to simplify this ...
pub fn run<E>(
    runner: &Runner,
    filter: &Filter,
    vars: Vars<Val>,
    inputs: impl Iterator<Item = Result<Val, impl ToString>>,
    fi: impl Fn(String) -> E,
    mut f: impl FnMut(ValX) -> Result<(), E>,
) -> Result<(), E> {
    let inputs = Box::new(inputs.map(|r| r.map_err(|e| e.to_string())));
    let null = Box::new(core::iter::once(Ok(Val::Null)));

    let null = &RcIter::new(null);

    let data = Data {
        runner,
        lut: &filter.lut,
        inputs: &RcIter::new(inputs),
        filename: &|| None,
        line_number: &|| 0,
    };
    let ctx = Ctx::new(&data, vars);

    let outputs = |x| filter.id.run((ctx.clone(), x));
    (if runner.null_input { null } else { data.inputs }).try_for_each(|x| match x {
        Ok(x) => outputs(x).try_for_each(&mut f),
        Err(e) => Err(fi(e)),
    })
}
