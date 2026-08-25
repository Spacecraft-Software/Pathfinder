// SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A token scanner for jq programs.
//!
//! Pathfinder has to answer three questions about a filter before it can hand
//! it to jaq: which polyfills does it need, where does its `import`/`include`
//! header end, and where does `$__loc__` appear. All three are lexical, so this
//! is a scanner and not a parser — Pathfinder never needs to know what the
//! program *means*, only which names it mentions and in what position.
//!
//! A substring search would be wrong in three separate ways, and all three
//! occur in real filters:
//!
//! - `jq '"tostream"'` mentions the name inside a string literal.
//! - `jq '. # tostream'` mentions it in a comment.
//! - `jq '.tostream'` is a field access, not a function call.
//!
//! The interesting case is string interpolation: `"\(.a)"` re-enters *code*
//! context inside a string, and can nest arbitrarily (`"\("\(.x)")"`). The
//! scanner tracks that with a stack rather than a flag.

/// Where a name was found, which decides whether it can be a function call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Prev {
    /// Start of input, or any character that can precede a call.
    Other,
    /// A `.`, so the next identifier is a field name (`.tostream`).
    Dot,
    /// A `$`, so the next identifier is a variable (`$tostream`).
    Dollar,
    /// The `def` keyword, so the next identifier is being defined.
    Def,
}

/// What the scanner found in a jq program.
#[derive(Debug, Default, Clone)]
pub struct Scan {
    /// Identifiers appearing in call position: not after `.`, `$`, or `def`.
    pub called: Vec<String>,
    /// Names the program defines itself, via `def NAME`.
    pub defined: Vec<String>,
    /// `@format` tokens, without the `@`.
    pub formats: Vec<String>,
    /// Byte offsets of every `$__loc__` occurrence, with its 1-based line.
    pub loc_sites: Vec<(usize, usize)>,
    /// Byte offset at which the leading `import`/`include` header ends.
    ///
    /// Zero when the program has no header. Everything before this offset must
    /// stay at the very top of the program jaq receives.
    pub header_end: usize,
    /// True when the program's final token is a `#` comment with no trailing
    /// newline, so wrapping it in parentheses would swallow the closing paren.
    pub ends_in_comment: bool,
    /// True when the program is empty or contains only whitespace and comments.
    pub is_effectively_empty: bool,
}

impl Scan {
    /// Whether `name` is called somewhere and not defined by the program itself.
    ///
    /// A program that defines the name still gets the polyfill injected — its
    /// own later `def` wins under jq's and jaq's last-definition-wins rule — but
    /// knowing the difference lets `--explain` say so.
    pub fn calls(&self, name: &str) -> bool {
        self.called.iter().any(|n| n == name)
    }

    /// Whether the program defines `name` itself.
    pub fn defines(&self, name: &str) -> bool {
        self.defined.iter().any(|n| n == name)
    }
}

const fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_'
}

const fn is_ident_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// Scan a jq program.
///
/// Never fails: a malformed program is jaq's problem to report, and the scanner
/// only has to avoid claiming a name is called when it is not. An unterminated
/// string literal simply means everything after it is treated as string content,
/// which yields no false positives.
#[expect(
    clippy::too_many_lines,
    reason = "one state machine; splitting it would hide which byte drives which transition"
)]
pub fn scan(src: &str) -> Scan {
    let b = src.as_bytes();
    let mut out = Scan::default();
    let mut i = 0usize;
    let mut line = 1usize;
    let mut prev = Prev::Other;
    let mut in_string = false;
    let mut paren_depth = 0usize;
    // Paren depth recorded on entering each `\(`; returning to it re-enters the
    // enclosing string literal.
    let mut interp: Vec<usize> = Vec::new();
    let mut saw_code = false;
    let mut last_was_comment = false;

    while i < b.len() {
        let c = b[i];
        if c == b'\n' {
            line += 1;
            last_was_comment = false;
            i += 1;
            continue;
        }

        if in_string {
            match c {
                b'\\' => {
                    if b.get(i + 1) == Some(&b'(') {
                        interp.push(paren_depth);
                        paren_depth += 1;
                        in_string = false;
                        i += 2;
                    } else {
                        // Any other escape consumes its payload byte, so a `\"`
                        // cannot end the literal.
                        i += 2;
                    }
                }
                b'"' => {
                    in_string = false;
                    prev = Prev::Other;
                    i += 1;
                }
                _ => i += 1,
            }
            continue;
        }

        match c {
            b'#' => {
                // Comment to end of line. jq has no block comments.
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
                last_was_comment = true;
            }
            b'"' => {
                in_string = true;
                saw_code = true;
                last_was_comment = false;
                i += 1;
            }
            b'(' => {
                paren_depth += 1;
                prev = Prev::Other;
                saw_code = true;
                last_was_comment = false;
                i += 1;
            }
            b')' => {
                paren_depth = paren_depth.saturating_sub(1);
                if interp.last() == Some(&paren_depth) {
                    interp.pop();
                    in_string = true;
                }
                prev = Prev::Other;
                saw_code = true;
                last_was_comment = false;
                i += 1;
            }
            b'.' => {
                prev = Prev::Dot;
                saw_code = true;
                last_was_comment = false;
                i += 1;
            }
            b'$' => {
                prev = Prev::Dollar;
                saw_code = true;
                last_was_comment = false;
                i += 1;
                // `$__loc__` is a variable, but it is the one variable Pathfinder
                // has to rewrite, so record it here where its span is known.
                if src[i..].starts_with("__loc__") {
                    out.loc_sites.push((i - 1, line));
                }
            }
            b'@' => {
                let start = i + 1;
                let mut j = start;
                while j < b.len() && is_ident_char(b[j]) {
                    j += 1;
                }
                if j > start {
                    out.formats.push(src[start..j].to_owned());
                }
                prev = Prev::Other;
                saw_code = true;
                last_was_comment = false;
                i = j.max(i + 1);
            }
            _ if is_ident_start(c) => {
                let start = i;
                let mut j = i;
                while j < b.len() && is_ident_char(b[j]) {
                    j += 1;
                }
                let word = &src[start..j];
                saw_code = true;
                last_was_comment = false;
                match prev {
                    Prev::Def => out.defined.push(word.to_owned()),
                    Prev::Dot | Prev::Dollar => {}
                    Prev::Other => {
                        if word == "def" {
                            prev = Prev::Def;
                            i = j;
                            continue;
                        }
                        out.called.push(word.to_owned());
                    }
                }
                prev = Prev::Other;
                i = j;
            }
            _ => {
                if !c.is_ascii_whitespace() {
                    prev = Prev::Other;
                    saw_code = true;
                    last_was_comment = false;
                }
                i += 1;
            }
        }
    }

    out.ends_in_comment = last_was_comment;
    out.is_effectively_empty = !saw_code;
    out.header_end = header_end(src);
    out
}

/// Find where the leading run of `import`/`include` directives ends.
///
/// jq (and jaq) require module directives to precede everything else, so
/// anything Pathfinder prepends has to go *after* them, not before. Returns the
/// byte offset just past the final directive's `;`, or 0 when there is no header.
fn header_end(src: &str) -> usize {
    let b = src.as_bytes();
    let mut i = 0usize;
    let mut end = 0usize;

    loop {
        // Skip whitespace and comments between directives.
        while i < b.len() {
            if b[i].is_ascii_whitespace() {
                i += 1;
            } else if b[i] == b'#' {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            } else {
                break;
            }
        }
        let rest = &src[i..];
        let keyword = if rest.starts_with("import") {
            "import"
        } else if rest.starts_with("include") {
            "include"
        } else {
            return end;
        };
        // Require a real token boundary so an `includes` function call is not
        // mistaken for an `include` directive.
        match b.get(i + keyword.len()) {
            Some(&next) if !is_ident_char(next) => {}
            _ => return end,
        }
        // Scan to the terminating `;`, skipping string literals so a `;` inside
        // a module path does not end the directive early.
        let mut j = i + keyword.len();
        let mut in_string = false;
        while j < b.len() {
            match b[j] {
                b'\\' if in_string => j += 1,
                b'"' => in_string = !in_string,
                b';' if !in_string => {
                    j += 1;
                    break;
                }
                _ => {}
            }
            j += 1;
        }
        if j > b.len() {
            return end;
        }
        end = j;
        i = j;
    }
}

#[cfg(test)]
mod tests {
    use super::scan;

    #[test]
    fn finds_names_in_call_position() {
        let s = scan("tostream | fromstream");
        assert!(s.calls("tostream"));
        assert!(s.calls("fromstream"));
    }

    #[test]
    fn ignores_names_inside_string_literals() {
        assert!(!scan(r#""tostream""#).calls("tostream"));
        assert!(!scan(r#"{"k": "tostream is a word"}"#).calls("tostream"));
    }

    #[test]
    fn ignores_names_inside_comments() {
        assert!(!scan("# tostream\n.").calls("tostream"));
        assert!(!scan(". # tostream").calls("tostream"));
    }

    #[test]
    fn ignores_field_access_and_variables() {
        assert!(!scan(".tostream").calls("tostream"));
        assert!(!scan("$tostream").calls("tostream"));
        assert!(!scan(".a.tostream").calls("tostream"));
    }

    #[test]
    fn sees_through_string_interpolation() {
        // The interpolation is code again, so a call inside it is a real call.
        let s = scan(r#""value: \(tostream)""#);
        assert!(s.calls("tostream"));
    }

    #[test]
    fn handles_nested_interpolation() {
        let s = scan(r#""a\("b\(tostream)c")d""#);
        assert!(s.calls("tostream"));
        // The literal segments are still strings.
        assert!(!s.calls("b"));
        assert!(!s.calls("d"));
    }

    #[test]
    fn escaped_quote_does_not_end_the_literal() {
        assert!(!scan(r#""he said \"tostream\" loudly""#).calls("tostream"));
    }

    #[test]
    fn records_definitions_separately_from_calls() {
        let s = scan("def tostream: 1; tostream");
        assert!(s.defines("tostream"));
        assert!(s.calls("tostream"));
    }

    #[test]
    fn records_format_tokens() {
        let s = scan("@base32");
        assert_eq!(s.formats, ["base32"]);
        assert!(
            scan(r#"@base64 "x""#)
                .formats
                .contains(&"base64".to_owned())
        );
    }

    #[test]
    fn records_loc_sites_with_line_numbers() {
        let s = scan("$__loc__");
        assert_eq!(s.loc_sites, [(0, 1)]);
        let s = scan(".a\n| $__loc__");
        assert_eq!(s.loc_sites.len(), 1);
        assert_eq!(s.loc_sites[0].1, 2);
    }

    #[test]
    fn finds_the_module_header_boundary() {
        let src = r#"include "foo"; import "bar" as b; .a"#;
        let s = scan(src);
        assert_eq!(&src[..s.header_end], r#"include "foo"; import "bar" as b;"#);
    }

    #[test]
    fn no_header_means_offset_zero() {
        assert_eq!(scan(".a | .b").header_end, 0);
        // An identifier that merely starts with "include" is not a directive.
        assert_eq!(scan("includes(1)").header_end, 0);
    }

    #[test]
    fn detects_effectively_empty_programs() {
        assert!(scan("").is_effectively_empty);
        assert!(scan("   \n  ").is_effectively_empty);
        assert!(scan("# just a comment").is_effectively_empty);
        assert!(!scan(".").is_effectively_empty);
    }

    #[test]
    fn detects_a_trailing_comment() {
        // Wrapping this in parens without a newline would eat the `)`.
        assert!(scan(". # done").ends_in_comment);
        assert!(!scan(". # done\n").ends_in_comment);
        assert!(!scan(".").ends_in_comment);
    }
}
