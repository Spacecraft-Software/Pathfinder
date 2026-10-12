//! Filter parsing, compilation, and execution.
use crate::{read, Error, Runner, Val};
use jaq_all::data::Filter;
use jaq_all::jaq_core::{compile, load, Exn, ValT, Vars};
use jaq_all::load::{compile_errors, load_errors, FileReports};
use std::{io, path::PathBuf};

pub fn parse_compile(
    path: &PathBuf,
    code: &str,
    vars: &[String],
    paths: &[PathBuf],
) -> Result<(Vec<Val>, Filter), Vec<FileReports<PathBuf>>> {
    use compile::Compiler;
    use load::{import, Arena, File, Loader};

    let default = ["~/.jq", "$ORIGIN/../lib/jq", "$ORIGIN/../lib"].map(|x| x.into());
    let paths = if paths.is_empty() { &default } else { paths };

    let vars: Vec<_> = vars.iter().map(|v| format!("${v}")).collect();
    let arena = Arena::default();
    let loader = Loader::new(jaq_all::defs()).with_std_read(paths);
    //let loader = Loader::new([]).with_std_read(paths);
    let path = path.into();
    let modules = loader
        .load(&arena, File { path, code })
        .map_err(load_errors)?;

    let mut vals = Vec::new();
    import(&modules, |p| {
        let path = p.find(paths, "json")?;
        vals.push(read::json_array(path).map_err(|e| e.to_string())?);
        Ok(())
    })
    .map_err(load_errors)?;

    let compiler = Compiler::default()
        .with_funs(jaq_all::data::funs())
        .with_global_vars(vars.iter().map(|v| &**v));
    let filter = compiler.compile(modules).map_err(compile_errors)?;
    Ok((vals, filter))
}

/// Pathfinder: a filter run as jq 1.8.1's `main` runs it.
///
/// An uncaught error is reported on stderr with `location`, ends only that
/// input's outputs, and is recorded in `status`, which decides the exit code.
/// No input is taken once `failed`: an input file could not be read.
pub(crate) fn run_jq(
    runner: &Runner,
    filter: &Filter,
    vars: Vars<Val>,
    inputs: impl Iterator<Item = io::Result<Val>>,
    exit_status: bool,
    status: &mut crate::jq::Status,
    location: &dyn Fn() -> String,
    filename: &dyn Fn() -> Option<String>,
    failed: &dyn Fn() -> bool,
    mut f: impl FnMut(Val) -> io::Result<()>,
) -> Result<(), Error> {
    use crate::jq::Ret;
    use io::Write;
    use jaq_all::data::{Ctx, Data};
    use jaq_all::jaq_std::input::RcIter;

    let inputs = Box::new(inputs.map(|r| r.map_err(|e| e.to_string())));
    let null = Box::new(core::iter::once(Ok(Val::Null)));
    let null = &RcIter::new(null);
    let data = Data {
        runner,
        lut: &filter.lut,
        inputs: &RcIter::new(inputs),
        filename,
    };
    let ctx = Ctx::new(&data, vars);
    let mut inputs = if runner.null_input { null } else { data.inputs };
    // Pathfinder: once a file could not be read, jq takes no further input.
    while !failed() {
        let Some(x) = inputs.next() else { break };
        let x = x.map_err(Error::Parse)?;
        let mut ret = Ret::NoOutput;
        for y in filter.id.run((ctx.clone(), x)) {
            match y.map_err(Exn::get_err) {
                Ok(v) => {
                    ret = if !exit_status || v.as_bool() {
                        Ret::Ok
                    } else {
                        Ret::NullKind
                    };
                    f(v)?;
                }
                Err(Ok(e)) => {
                    io::stderr().write_all(&crate::jq::report(e, &location()))?;
                    ret = Ret::Error;
                    break;
                }
                Err(Err(e)) => return Err(Error::Halt(e.get_halt().ok().unwrap())),
            }
        }
        status.end(ret);
    }
    Ok(())
}
