// SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Assembly of the program text jaq actually receives.
//!
//! Four things can be spliced into a user's filter, and they have to happen in
//! one place because their ordering constraints interact:
//!
//! 1. `import`/`include` directives must stay at the very top — jaq rejects a
//!    directive that follows a `def`.
//! 2. The polyfill prelude must precede any use of the names it defines.
//! 3. A `$ARGS` wrap must enclose the user's filter but sit below the directives.
//! 4. `$__loc__` is a literal jaq does not know, so each occurrence is
//!    substituted in place.
//!
//! Assembling these separately would produce a program that is subtly wrong in
//! ways that only show up on filters using modules, which is exactly the
//! population least likely to be covered by a quick test.
//!
//! The prelude is emitted on the *same line* as the filter's first line. jaq
//! reports errors against the text it was handed, so a prelude on its own line
//! would shift every reported line number by one; keeping it inline leaves the
//! user's line numbers untouched and only moves column numbers on line 1.

use std::fmt::Write as _;

use crate::prelude;
use crate::scan::{self, Scan};

/// The program to hand to jaq, plus what had to be done to it.
#[derive(Debug, Clone)]
pub struct Assembly {
    /// The final program text.
    pub text: String,
    /// Polyfill and repair names injected, in injection order.
    pub injected: Vec<String>,
    /// Names the filter uses that cannot be supplied at all, with the reason.
    pub inexpressible: Vec<(String, &'static str)>,
    /// Injected names the filter also defines itself. Its own definition wins,
    /// which is correct but worth being able to point at when a polyfill
    /// appears not to take effect.
    pub shadowed: Vec<String>,
    /// False when `text` is byte-identical to the user's filter, which means the
    /// invocation can keep `-f` and stay on the untouched fast path.
    pub rewritten: bool,
}

/// A binding to wrap the filter in, as the text before the parenthesised body.
///
/// For `--jsonargs` this rewrites jaq's own `$ARGS` rather than replacing it —
/// building the object from scratch would silently drop `$ARGS.named` from
/// `--arg`/`--argjson`, and leaving it untouched would leak the internal
/// bindings the emulation had to add.
#[derive(Debug, Clone)]
pub struct Wrap(pub String);

/// Assemble the final program.
///
/// `loc_file` is what `$__loc__.file` should report. jq answers `"<top-level>"`
/// for the main program whether it came from argv or from `-f`; only a filter
/// inside an included module reports a path.
pub fn assemble(
    src: &str,
    ctx: &prelude::Context,
    wrap: Option<&Wrap>,
    loc_file: &str,
) -> Assembly {
    let info = scan::scan(src);

    let wanted: Vec<String> = prelude::known_names()
        .into_iter()
        .filter(|n| info.calls(n))
        .map(str::to_owned)
        .collect();
    let prelude_text = prelude::render(&wanted, ctx);

    let inexpressible: Vec<(String, &'static str)> = info
        .called
        .iter()
        .filter_map(|n| prelude::inexpressible(n).map(|why| (n.clone(), why)))
        .collect();

    let header = &src[..info.header_end];
    let body = substitute_loc(src, &info, loc_file);
    let body = body.trim_start_matches(|c: char| c.is_ascii_whitespace());

    // jq 1.7+ treats an empty filter as the identity; `X as $ARGS | ( )` is a
    // syntax error, so normalize before wrapping.
    let body: &str = if info.is_effectively_empty { "." } else { body };

    let mut text = String::with_capacity(src.len() + prelude_text.len() + 64);
    if !header.is_empty() {
        text.push_str(header);
        text.push('\n');
    }
    if !prelude_text.is_empty() {
        text.push_str(&prelude_text);
        text.push(' ');
    }
    match wrap {
        Some(Wrap(binding)) => {
            text.push_str(binding);
            text.push_str(" (");
            text.push_str(body);
            // A filter ending in a `#` comment would otherwise swallow the
            // closing parenthesis.
            if info.ends_in_comment {
                text.push('\n');
            }
            text.push(')');
        }
        None => text.push_str(body),
    }

    let shadowed: Vec<String> = wanted.iter().filter(|n| info.defines(n)).cloned().collect();

    let rewritten = text != src;
    Assembly {
        text,
        injected: wanted,
        inexpressible,
        shadowed,
        rewritten,
    }
}

/// Replace every `$__loc__` with the object jq would produce there.
///
/// `$__loc__` is a literal, not a function, so it cannot be a `def`. jq
/// documents it as an object with `file` and `line` keys, both of which
/// Pathfinder knows at assembly time.
fn substitute_loc(src: &str, info: &Scan, loc_file: &str) -> String {
    const TOKEN_LEN: usize = "$__loc__".len();
    if info.loc_sites.is_empty() {
        return src[info.header_end..].to_owned();
    }
    let mut out = String::with_capacity(src.len());
    let mut cursor = info.header_end;
    for &(offset, line) in &info.loc_sites {
        if offset < cursor {
            continue;
        }
        out.push_str(&src[cursor..offset]);
        let _ = write!(out, r#"{{"file":"{}","line":{}}}"#, escape(loc_file), line);
        cursor = offset + TOKEN_LEN;
    }
    out.push_str(&src[cursor..]);
    out
}

/// Escape a path for embedding in a JSON string literal.
fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::{Wrap, assemble};
    use crate::prelude::Context;

    fn plain(src: &str) -> super::Assembly {
        assemble(src, &Context::default(), None, "<top-level>")
    }

    #[test]
    fn an_ordinary_filter_is_passed_through_untouched() {
        let a = plain(".a | select(.b)");
        assert_eq!(a.text, ".a | select(.b)");
        assert!(
            !a.rewritten,
            "no rewrite means the exec fast path stays available"
        );
        assert!(a.injected.is_empty());
    }

    #[test]
    fn a_polyfilled_name_pulls_in_its_definition() {
        let a = plain("[tostream]");
        assert!(a.rewritten);
        assert!(a.text.contains("def tostream:"));
        assert!(a.text.ends_with("[tostream]"));
        assert_eq!(a.injected, ["tostream"]);
    }

    #[test]
    fn the_prelude_stays_on_line_one() {
        let a = plain("[tostream]\n| .[0]");
        let first = a.text.lines().next().expect("has a first line");
        assert!(first.contains("def tostream:"));
        assert!(
            first.contains("[tostream]"),
            "filter's line 1 must stay on line 1"
        );
        // The user's `| .[0]` was on line 2 and must still be on line 2.
        assert_eq!(a.text.lines().count(), 2);
    }

    #[test]
    fn module_directives_stay_above_everything() {
        let a = plain(r#"include "m"; [tostream]"#);
        assert!(a.text.starts_with(r#"include "m";"#));
        let dir = a.text.find("include").expect("directive present");
        let def = a.text.find("def tostream:").expect("prelude present");
        assert!(dir < def, "a def before a directive does not compile");
    }

    #[test]
    fn the_wrap_encloses_the_body_but_not_the_directives() {
        let wrap = Wrap("($ARGS + {}) as $ARGS |".to_owned());
        let a = assemble(
            r#"include "m"; .a"#,
            &Context::default(),
            Some(&wrap),
            "<top-level>",
        );
        let dir = a.text.find("include").expect("directive present");
        let bind = a.text.find("as $ARGS").expect("wrap present");
        assert!(dir < bind);
        assert!(a.text.ends_with("(.a)"));
    }

    #[test]
    fn an_empty_filter_becomes_the_identity_before_wrapping() {
        let wrap = Wrap("(1) as $ARGS |".to_owned());
        let a = assemble("", &Context::default(), Some(&wrap), "<top-level>");
        // `... | ( )` would be a syntax error.
        assert!(a.text.ends_with("(.)"));
    }

    #[test]
    fn a_trailing_comment_does_not_swallow_the_closing_paren() {
        let wrap = Wrap("(1) as $ARGS |".to_owned());
        let a = assemble(". # done", &Context::default(), Some(&wrap), "<top-level>");
        assert!(
            a.text.ends_with("\n)"),
            "needs a newline to close the comment: {}",
            a.text
        );
    }

    #[test]
    fn loc_is_substituted_with_file_and_line() {
        let a = plain("$__loc__");
        assert_eq!(a.text, r#"{"file":"<top-level>","line":1}"#);
        let a = assemble(".a\n| $__loc__", &Context::default(), None, "<top-level>");
        assert!(a.text.contains(r#"{"file":"<top-level>","line":2}"#));
    }

    #[test]
    fn loc_inside_a_string_is_left_alone() {
        let a = plain(r#""$__loc__""#);
        assert_eq!(a.text, r#""$__loc__""#);
        assert!(!a.rewritten);
    }

    #[test]
    fn names_that_cannot_be_supplied_are_reported_not_faked() {
        let a = plain("input_line_number");
        assert_eq!(a.inexpressible.len(), 1);
        assert_eq!(a.inexpressible[0].0, "input_line_number");
        assert!(!a.text.contains("def input_line_number"));
    }

    #[test]
    fn a_user_definition_wins_over_the_polyfill() {
        // Both are present; jq and jaq both take the last definition, so the
        // user's own wins without Pathfinder having to suppress anything.
        let a = plain("def tostream: \"mine\"; tostream");
        let ours = a
            .text
            .find("def tostream: path(")
            .expect("polyfill injected");
        let theirs = a
            .text
            .find(r#"def tostream: "mine""#)
            .expect("user def kept");
        assert!(ours < theirs);
        assert_eq!(a.shadowed, ["tostream"], "explain should be able to say so");
    }
}
