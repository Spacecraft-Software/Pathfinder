#![expect(
    clippy::default_trait_access,
    clippy::deref_by_slicing,
    clippy::redundant_closure_for_method_calls,
    clippy::semicolon_if_nothing_returned,
    clippy::similar_names,
    clippy::too_many_arguments,
    redundant_imports,
    reason = "jaq 3.1.1's CLI with the patch set's CLI hunks (crates/UPSTREAM.md): a test vehicle that M19 retires, kept to the packaged engine's source so it stays a faithful stand-in for it"
)]
#![expect(
    clippy::needless_pass_by_value,
    clippy::renamed_function_params,
    clippy::trivially_copy_pass_by_ref,
    reason = "jaq 3.1.1's CLI with the patch set's CLI hunks (crates/UPSTREAM.md): a test vehicle that M19 retires, kept to the packaged engine's source so it stays a faithful stand-in for it"
)]
#![expect(
    clippy::cast_possible_truncation,
    reason = "jaq 3.1.1's CLI with the patch set's CLI hunks (crates/UPSTREAM.md): a test vehicle that M19 retires, kept to the packaged engine's source so it stays a faithful stand-in for it"
)]
#![expect(
    clippy::multiple_crate_versions,
    reason = "syn 2 (jiff-static, under jiff) and syn 3 (serde_derive) are both build-time proc-macro dependencies of third-party crates"
)]

mod cli;
mod filter;
mod jq;
mod tests;

use cli::{Cli, Format};
use core::fmt::{self, Display, Formatter};
use jaq_all::data::{Filter, Runner};
use jaq_all::fmts::read;
use jaq_all::fmts::write::{with_stdout, write, Writer};
use jaq_all::jaq_core::Vars;
use jaq_all::json::write::{Pp, Styles};
use jaq_all::json::Val;
use jaq_all::load::{Color, FileReports, FileReportsDisp, Paint};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{ExitCode, Termination};

#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

extern crate alloc;

fn main() -> ExitCode {
    use env_logger::Env;
    env_logger::Builder::from_env(Env::default().filter_or("LOG", "jaq=debug"))
        .format(|buf, record| match record.level() {
            // format error messages (yielded by `stderr`) without newline
            log::Level::Error => write!(buf, "{}", record.args()),
            // format debug messages such as `["DEBUG:", [1, 2, 3]]`
            // Pathfinder: jq 1.8.1 writes it compactly, `["DEBUG:",[1,2,3]]`.
            level => writeln!(buf, "[\"{}:\",{}]", level, record.args()),
        })
        .init();

    // last line of defense against I/O errors that happen during printing,
    // e.g. catching broken pipes during error printing
    main_io(&mut io::stdin(), &mut io::stdout(), &mut io::stderr())
        .unwrap_or_else(|e: io::Error| Error::from(e).report())
}

fn main_io(
    inp: &mut io::Stdin,
    out: &mut io::Stdout,
    err: &mut io::Stderr,
) -> io::Result<ExitCode> {
    let cli = match Cli::parse() {
        Ok(cli) => cli,
        Err(e) => {
            writeln!(err, "Error: {e}")?;
            return Ok(ExitCode::from(2));
        }
    };

    if cli.version {
        let name = env!("CARGO_PKG_NAME");
        // Pathfinder's patch set: number output, arithmetic precision, input
        // literals, string repetition, object keys, `debug`, uncaught errors,
        // `halt_error`, lazy input and file names in error locations as jq
        // 1.8.1 has them.
        let version = concat!(env!("CARGO_PKG_VERSION"), "+pathfinder.3");
        writeln!(out, "{name} {version}")?;
        Ok(ExitCode::SUCCESS)
    } else if cli.help {
        writeln!(out, "{}", include_str!("help.txt"))?;
        Ok(ExitCode::SUCCESS)
    } else if let Some(test_files) = &cli.run_tests {
        match test_files.last() {
            Some(file) => tests::run(io::BufReader::new(std::fs::File::open(file)?)),
            None => tests::run(inp.lock()),
        }
    } else {
        real_main(&cli, inp, out).or_else(|e| {
            write!(err, "{}", ErrorColor::new(&e, cli.color_errors()))?;
            Ok(e.report())
        })
    }
}

impl Cli {
    fn runner(&self) -> Runner {
        Runner {
            null_input: self.null_input,
            color_err: self.color_errors(),
            writer: self.writer(),
        }
    }

    fn writer(&self) -> Writer {
        Writer {
            pp: self.pp(),
            format: self.to.unwrap_or_default(),
            join: self.join_output,
        }
    }

    fn pp(&self) -> Pp {
        Pp {
            indent: (!self.compact_output).then(|| self.indent()),
            sort_keys: self.sort_keys,
            styles: self.styles(),
            sep_space: !self.compact_output,
        }
    }

    fn styles(&self) -> Styles {
        self.color_output()
            .then(Styles::ansi)
            .map(|c| match std::env::var("JQ_COLORS") {
                Err(_) => c,
                Ok(s) => c.parse(&s),
            })
            .unwrap_or_default()
    }
}

fn real_main(cli: &Cli, inp: &mut io::Stdin, out: &mut io::Stdout) -> Result<ExitCode, Error> {
    // Pathfinder: the files Pathfinder concatenated onto stdin, taken out of
    // the environment before `$ENV` is made from it.
    let segments = jq::Segment::take_env();
    let mut var_val = binds(cli)?;
    let input_filename_idx = var_val.len();
    var_val.push(("!input_filename".to_string(), Val::Null));
    let (var_names, mut vars): (Vec<String>, Vec<Val>) = var_val.into_iter().unzip();

    let (var_vals, filter) = match &cli.filter {
        None => (Vec::new(), Filter::default()),
        Some(filter) => {
            let (path, code) = match filter {
                cli::Filter::FromFile(path) => (path.into(), std::fs::read_to_string(path)?),
                cli::Filter::Inline(filter) => ("<inline>".into(), filter.clone()),
            };
            filter::parse_compile(&path, &code, &var_names, &cli.library_path)
                .map_err(Error::Report)?
        }
    };
    vars.extend(var_vals);
    //println!("Filter: {:?}", filter);

    let runner = &cli.runner();
    let writer = &runner.writer;

    let unwrap_or_json = |fmt: Option<Format>| fmt.unwrap_or_default();
    // Pathfinder: how the inputs ended, for jq's exit status.
    let mut status = jq::Status::default();
    if cli.files.is_empty() {
        vars[input_filename_idx] = Val::utf8_str("<stdin>");
        let vars = Vars::new(vars);

        let format = unwrap_or_json(cli.from);
        let pos = jq::Position::with_segments(segments);
        let lines = jq::Lines::new(inp.lock(), &pos);
        // Pathfinder: JSON is read as jq reads it, with jq's parse errors.
        let inputs = match format {
            Format::Json => {
                read::collect_if(cli.slurp, jaq_all::json::read::read_many_jq(lines, &pos))
            }
            _ => read::read(format, lines, cli.slurp),
        };
        let location = || pos.location("<stdin>", None);
        let failed = || pos.failed();
        let (es, st) = (cli.exit_status, &mut status);
        let ran = with_stdout(out, |out| {
            filter::run_jq(
                runner,
                &filter,
                vars,
                inputs,
                es,
                st,
                &location,
                &failed,
                |v| write(out, writer, &v),
            )
        });
        // Pathfinder: a file that could not be read makes jq exit with 2,
        // whatever else happened, `halt_error` included.
        if pos.failed() {
            if let Err(e) = ran {
                eprint!("{}", ErrorColor::new(&e, cli.color_errors()));
            }
            return Ok(ExitCode::from(2));
        }
        ran?;
    } else {
        for file in &cli.files {
            vars[input_filename_idx] =
                Val::utf8_str(file.as_os_str().to_string_lossy().into_owned());
            let vars = Vars::new(vars.clone());

            let path = Path::new(file);
            let bytes = read::load_file(path)
                .map_err(|e| Error::Io(Some(path.display().to_string()), e))?;
            // Pathfinder: JSON whatever the file's extension, as jq reads it.
            let format = unwrap_or_json(cli.from);

            // Pathfinder: JSON is parsed in place, recording how far it
            // has been read; raw text is read a line at a time.
            let pos = jq::Position::default();
            let inputs = match format {
                Format::Json => read::collect_if(
                    cli.slurp,
                    jaq_all::json::read::parse_many_at(&bytes, pos.at())
                        .map(|r| r.map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))),
                ),
                _ => read::read(format, jq::Lines::new(&bytes[..], &pos), cli.slurp),
            };
            let name = file.as_os_str().to_string_lossy();
            let in_memory = matches!(format, Format::Json).then_some(&bytes[..]);
            let location = || pos.location(&name, in_memory);
            let (es, st) = (cli.exit_status, &mut status);
            let failed = || false;
            with_stdout(out, |out| {
                filter::run_jq(
                    runner,
                    &filter,
                    vars.clone(),
                    inputs,
                    es,
                    st,
                    &location,
                    &failed,
                    |v| write(out, writer, &v),
                )
            })?;
        }
    }

    // Pathfinder: jq's exit status, decided by the last input.
    Ok(ExitCode::from(status.code(cli.exit_status)))
}

fn binds(cli: &Cli) -> Result<Vec<(String, Val)>, Error> {
    let arg = cli.arg.iter().map(|(k, s)| {
        let s = s.to_owned();
        Ok((k.to_owned(), Val::utf8_str(s)))
    });
    // Pathfinder: jq 1.8.1 reads exactly one value and words the error as
    // below. Pathfinder passes `--jsonargs` values as `--argjson` bindings
    // named `__pathfinder_p<N>`, which jq reports as `--jsonargs`.
    let argjson = cli.argjson.iter().map(|(k, s)| {
        let flag = if k.starts_with("__pathfinder_p") {
            "jsonargs"
        } else {
            "argjson"
        };
        let err = |_| Error::Usage(format!("invalid JSON text passed to --{flag}"));
        let s = s.as_bytes();
        Ok((k.to_owned(), read::json::parse_jq_single(s).map_err(err)?))
    });
    // Pathfinder: jq 1.8.1's report of a file it cannot open or parse.
    let bad = |which: &str, k: &str, path: &Path, e: io::Error| {
        let path = path.display();
        let e = match e.kind() {
            io::ErrorKind::InvalidData => e.to_string(),
            _ => format!("Could not open {path}: {}", strerror(&e)),
        };
        Error::System(format!("Bad JSON in --{which} {k} {path}: {e}"))
    };
    let rawfile = cli.rawfile.iter().map(|(k, path)| {
        let s = read::load_file(path).map_err(|e| bad("rawfile", k, path.as_ref(), e))?;
        Ok((k.to_owned(), Val::utf8_str(s)))
    });
    let slurpfile = cli.slurpfile.iter().map(|(k, path)| {
        let v = read::json_array(path).map_err(|e| bad("slurpfile", k, path.as_ref(), e))?;
        Ok((k.to_owned(), v))
    });

    let positional = cli.args.iter().cloned().map(|s| Ok(Val::from(s)));
    let positional = positional.collect::<Result<Vec<_>, Error>>()?;

    let var_val = arg.chain(rawfile).chain(slurpfile).chain(argjson);
    let mut var_val = var_val.collect::<Result<Vec<_>, Error>>()?;

    var_val.push(("ARGS".to_string(), args(&positional, &var_val)));
    let env = std::env::vars().map(|(k, v)| (k.into(), Val::from(v)));
    var_val.push(("ENV".to_string(), Val::obj(env.collect())));

    Ok(var_val)
}

/// Pathfinder: the C library's text for an I/O error, as jq prints it:
/// Rust's adds ` (os error N)`.
fn strerror(e: &io::Error) -> String {
    let s = e.to_string();
    match e.raw_os_error() {
        Some(n) => s
            .strip_suffix(&format!(" (os error {n})"))
            .map_or_else(|| s.clone(), str::to_string),
        None => s,
    }
}

fn args(positional: &[Val], named: &[(String, Val)]) -> Val {
    let key = |k: &str| k.to_string().into();
    let positional = positional.iter().cloned();
    let named = named.iter().map(|(var, val)| (key(var), val.clone()));
    let obj = [
        (key("positional"), positional.collect()),
        (key("named"), Val::obj(named.collect())),
    ];
    Val::obj(obj.into_iter().collect())
}

#[derive(Debug)]
enum Error {
    Io(Option<String>, io::Error),
    Report(Vec<FileReports<PathBuf>>),
    Parse(String),
    Jaq(jaq_all::json::Error),
    Halt(i32),
    /// Pathfinder: jq's report of a bad command line, followed by its
    /// pointer to the help (`die` in jq's `main.c`), exit status 2.
    Usage(String),
    /// Pathfinder: jq's report of a system error, exit status 2.
    System(String),
    // Pathfinder: the exit status now comes from `jq::Status`.
    #[expect(dead_code, reason = "jaq's exit status 1, now decided by `jq::Status`")]
    FalseOrNull,
    #[expect(dead_code, reason = "jaq's exit status 4, now decided by `jq::Status`")]
    NoOutput,
}

struct ErrorColor<'e>(&'e Error, Paint);

impl<'e> ErrorColor<'e> {
    fn new(e: &'e Error, color: bool) -> Self {
        let with_color = |f: &mut Formatter, c: &Option<Color>, d: &dyn Display| match c {
            Some(color) => color.ansi(f, d),
            None => d.fmt(f),
        };
        let without_color = |f: &mut Formatter, _c: &_, d: &dyn Display| d.fmt(f);
        Self(e, if color { with_color } else { without_color })
    }
}

impl fmt::Display for ErrorColor<'_> {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        let Self(error, color) = self;
        match error {
            Error::FalseOrNull | Error::NoOutput | Error::Halt(_) => Ok(()),
            Error::Io(_, e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(()),
            Error::Io(prefix, e) => {
                write!(f, "Error: ")?;
                if let Some(p) = prefix {
                    write!(f, "{p}: ")?;
                }
                writeln!(f, "{e}")
            }
            Error::Report(reports) => reports.iter().try_for_each(|fr| {
                FileReportsDisp::new(fr)
                    .with_paint(*color)
                    .with_path(|p| format!("[{}]", p.display()))
                    .fmt(f)
            }),
            // Pathfinder: jq's report of a parse error in its input.
            Error::Parse(e) => writeln!(f, "jq: parse error: {e}"),
            Error::Jaq(e) => writeln!(f, "Error: {e}"),
            Error::Usage(e) => {
                writeln!(f, "jq: {e}")?;
                writeln!(f, "Use jq --help for help with command-line options,")?;
                writeln!(
                    f,
                    "or see the jq manpage, or online docs  at https://jqlang.org"
                )
            }
            Error::System(e) => writeln!(f, "jq: {e}"),
        }
    }
}

impl Termination for Error {
    fn report(self) -> ExitCode {
        ExitCode::from(match self {
            Self::FalseOrNull => 1,
            Self::Io(_, e) if e.kind() == io::ErrorKind::BrokenPipe => 141,
            Self::Io(_, _) | Self::Usage(_) | Self::System(_) => 2,
            Self::Report(_) => 3,
            Self::NoOutput => 4,
            Self::Parse(_) | Self::Jaq(_) => 5,
            // ExitCode ~= u8, but exit_code: i32
            Self::Halt(exit_code) => std::process::exit(exit_code),
        })
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Self::Io(None, e)
    }
}
