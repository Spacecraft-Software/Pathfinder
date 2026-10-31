// SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Several input files, read as the one stream jq reads them as.
//!
//! jq reads its files one after another as a single stream, with nothing
//! between them: `12` in one file and `34` in the next are the number `1234`.
//! The engine's driver runs the program once per file, so Pathfinder reads
//! the files as one stream instead (see `translate::concat_inputs`).
//!
//! jq still knows which file it is reading. An uncaught error is reported at
//! `a.json:3`, the line count starting again in each file, and a file that
//! cannot be opened is reported when the reader reaches it. So the files are
//! also described to the driver as [`jaq::Segment`]s: names, byte counts, and
//! the jq message for one that cannot be read.
//!
//! Byte counts have to be known before the engine starts. A regular file's is
//! its size; any other input that is not the last (stdin, a FIFO, a process
//! substitution) is read into memory first, and so is every input but the
//! last under `--seq`, whose separators are stripped. The last input needs no
//! count and is always streamed.

use std::ffi::OsString;
use std::fs::File;
use std::io::{self, Read};

use crate::post;

/// Where an input's bytes come from.
enum Source {
    /// A regular file, of which this many bytes are fed.
    File(File, u64),
    /// Read up front, separators already stripped if need be.
    Bytes(Vec<u8>),
    /// Pathfinder's own stdin, streamed (the last input only).
    Stdin,
    /// Nothing: the file could not be read.
    Failed(String),
}

/// One input file.
pub struct Input {
    /// The name jq reports it under.
    name: OsString,
    source: Source,
}

impl Input {
    /// jq's message for an input that could not be read, after `jq: error: `.
    pub fn failure(&self) -> Option<&str> {
        match &self.source {
            Source::Failed(m) => Some(m),
            _ => None,
        }
    }

    fn len(&self) -> u64 {
        match &self.source {
            Source::File(_, n) => *n,
            Source::Bytes(b) => b.len() as u64,
            Source::Stdin | Source::Failed(_) => 0,
        }
    }
}

/// Open every input, in order, before jaq starts.
///
/// jq opens each file only when it gets to it; opening them first changes
/// nothing it reports, because the failures are reported by jaq when its
/// reader reaches them.
pub fn open(files: &[OsString], strip: bool) -> Vec<Input> {
    let last = files.len().saturating_sub(1);
    files
        .iter()
        .enumerate()
        .map(|(i, path)| {
            let streamed = i == last;
            if path == "-" {
                let source = if streamed {
                    Source::Stdin
                } else {
                    read_all(&mut io::stdin().lock(), strip)
                };
                return Input {
                    name: OsString::from("<stdin>"),
                    source,
                };
            }
            let source = match File::open(path) {
                Err(e) => Source::Failed(format!(
                    "Could not open file {}: {}",
                    path.to_string_lossy(),
                    strerror(&e)
                )),
                Ok(mut file) => match file.metadata() {
                    // A directory opens but cannot be read: jq says why.
                    Ok(m) if m.is_dir() => read_all(&mut file, strip),
                    // A size of 0 is not trusted (`/proc` reports it).
                    Ok(m) if m.is_file() && (streamed || (!strip && m.len() > 0)) => {
                        Source::File(file, m.len())
                    }
                    _ if streamed => Source::File(file, u64::MAX),
                    _ => read_all(&mut file, strip),
                },
            };
            Input {
                name: path.clone(),
                source,
            }
        })
        .collect()
}

/// Read an input whole, so that its length is known.
fn read_all(input: &mut impl Read, strip: bool) -> Source {
    let mut bytes = Vec::new();
    let read = if strip {
        post::strip_rs(input, &mut bytes)
    } else {
        input.read_to_end(&mut bytes).map(|_| ())
    };
    match read {
        Ok(()) => Source::Bytes(bytes),
        Err(e) => Source::Failed(strerror(&e)),
    }
}

/// The C library's text for an error, as jq prints it: Rust's, without the
/// ` (os error N)` it appends.
fn strerror(e: &io::Error) -> String {
    let text = e.to_string();
    match (e.raw_os_error(), text.rfind(" (os error ")) {
        (Some(_), Some(i)) => text[..i].to_owned(),
        _ => text,
    }
}

/// The inputs as the engine's [`jaq::Segment`]s.
pub fn segments(inputs: &[Input]) -> Vec<jaq::Segment> {
    inputs
        .iter()
        .map(|i| {
            let name = i.name.to_string_lossy().into_owned();
            jaq::Segment::new(name, i.len(), i.failure().map(str::to_owned))
        })
        .collect()
}

/// The inputs one after another as one stream, with nothing between them, as
/// jq reads them; with RS stripped if `--seq` asked for it.
pub fn reader(inputs: Vec<Input>, strip: bool) -> impl Read {
    let parts: Vec<Box<dyn Read>> = inputs
        .into_iter()
        .filter_map(|input| -> Option<Box<dyn Read>> {
            let part: Box<dyn Read> = match input.source {
                // Exactly the length described, should the file grow. One
                // that shrinks meanwhile makes the stream shorter than
                // described: later locations name a file too early.
                Source::File(file, len) => Box::new(file.take(len)),
                Source::Bytes(bytes) => return Some(Box::new(io::Cursor::new(bytes))),
                Source::Stdin => Box::new(io::stdin().lock()),
                Source::Failed(_) => return None,
            };
            Some(if strip {
                Box::new(post::StripRs(part))
            } else {
                part
            })
        })
        .collect();
    Concat {
        parts: parts.into_iter(),
        current: None,
    }
}

/// Readers read one after another. Not nested `Read::chain`s, whose depth
/// would grow with the number of files.
struct Concat<I> {
    parts: I,
    current: Option<Box<dyn Read>>,
}

impl<I: Iterator<Item = Box<dyn Read>>> Read for Concat<I> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            if self.current.is_none() {
                match self.parts.next() {
                    Some(part) => self.current = Some(part),
                    None => return Ok(0),
                }
            }
            if let Some(part) = &mut self.current {
                let n = part.read(buf)?;
                if n > 0 {
                    return Ok(n);
                }
            }
            self.current = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::strerror;
    use std::io;

    #[test]
    fn strerror_drops_the_os_error_number() {
        let e = io::Error::from_raw_os_error(2);
        assert_eq!(strerror(&e), "No such file or directory");
    }
}
