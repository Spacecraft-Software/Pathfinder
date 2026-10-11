// SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The jq lexer, transcribed from jq 1.8.1's `src/lexer.l`.
//!
//! The rules that matter, and that a hand-written lexer gets wrong by default:
//!
//! - flex takes the **longest** match, and keywords are separate rules from
//!   identifiers. So `if` is a keyword but `iffy` is an identifier, `.if` is a
//!   field (two-character `.i…` beats the one-character `.`), and `x?//y`
//!   lexes as `x ?// y` — the alternative-destructuring token — which jq then
//!   rejects as a syntax error.
//! - `.5` is the number 0.5; `.e5` is the field `e5`.
//! - A comment runs to the end of the line, but a backslash before the newline
//!   continues it onto the next line (jq 1.7+).
//! - `\(` inside a string starts an interpolation that runs to the matching
//!   `)`, and may itself contain strings, parentheses and comments.

use super::Span;

/// A jq keyword.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kw {
    As,
    Import,
    Include,
    Module,
    Def,
    If,
    Then,
    Else,
    Elif,
    And,
    Or,
    End,
    Reduce,
    Foreach,
    Try,
    Catch,
    Label,
    Break,
}

impl Kw {
    fn from_word(w: &str) -> Option<Self> {
        Some(match w {
            "as" => Self::As,
            "import" => Self::Import,
            "include" => Self::Include,
            "module" => Self::Module,
            "def" => Self::Def,
            "if" => Self::If,
            "then" => Self::Then,
            "else" => Self::Else,
            "elif" => Self::Elif,
            "and" => Self::And,
            "or" => Self::Or,
            "end" => Self::End,
            "reduce" => Self::Reduce,
            "foreach" => Self::Foreach,
            "try" => Self::Try,
            "catch" => Self::Catch,
            "label" => Self::Label,
            "break" => Self::Break,
            _ => return None,
        })
    }

    /// The keyword's spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::As => "as",
            Self::Import => "import",
            Self::Include => "include",
            Self::Module => "module",
            Self::Def => "def",
            Self::If => "if",
            Self::Then => "then",
            Self::Else => "else",
            Self::Elif => "elif",
            Self::And => "and",
            Self::Or => "or",
            Self::End => "end",
            Self::Reduce => "reduce",
            Self::Foreach => "foreach",
            Self::Try => "try",
            Self::Catch => "catch",
            Self::Label => "label",
            Self::Break => "break",
        }
    }
}

/// One part of a string literal.
#[derive(Debug, Clone)]
pub enum StrPart {
    /// Literal text, as written (escapes included), between the quotes.
    Text(Span),
    /// A `\(…)` interpolation: its tokens, and the span of the whole `\(…)`.
    Interp(Vec<Token>, Span),
}

/// A token kind.
#[derive(Debug, Clone)]
pub enum Tok {
    Ident(String),
    Field(String),
    Binding(String),
    Loc,
    Format(String),
    Number,
    Str(Vec<StrPart>),
    Keyword(Kw),
    /// Operators and punctuation, longest match first.
    Op(&'static str),
    Eof,
}

/// A token and where it is.
#[derive(Debug, Clone)]
pub struct Token {
    pub tok: Tok,
    pub span: Span,
}

/// A lexing failure: input jq's lexer would also reject.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LexError(pub usize);

/// Operators, longest first so the scan takes the longest match like flex.
const OPS: &[&str] = &[
    "?//", "//=", "|=", "+=", "-=", "*=", "/=", "%=", "<=", ">=", "!=", "==", "//", "..", ".", "?",
    "=", ";", ",", ":", "|", "+", "-", "*", "/", "%", "$", "<", ">", "[", "]", "{", "}", "(", ")",
];

const fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_'
}

const fn is_ident_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// Tokenize a whole program.
pub fn lex(src: &str) -> Result<Vec<Token>, LexError> {
    let (mut toks, end) = lex_from(src, 0, false)?;
    toks.push(Token {
        tok: Tok::Eof,
        span: Span::new(end, end),
    });
    Ok(toks)
}

/// Tokenize from `pos`. With `interp`, stop at the `)` that closes the
/// interpolation and return the position just past it.
#[expect(
    clippy::too_many_lines,
    reason = "one arm per lexer.l rule; splitting would hide the correspondence"
)]
fn lex_from(src: &str, mut pos: usize, interp: bool) -> Result<(Vec<Token>, usize), LexError> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut depth = 0usize;
    loop {
        // Whitespace and comments.
        loop {
            match b.get(pos) {
                Some(c) if c.is_ascii_whitespace() => pos += 1,
                Some(b'#') => pos = skip_comment(b, pos),
                _ => break,
            }
        }
        let Some(&c) = b.get(pos) else {
            if interp {
                return Err(LexError(pos));
            }
            return Ok((out, pos));
        };
        let start = pos;

        if c == b'"' {
            let (parts, end) = lex_string(src, pos + 1)?;
            out.push(Token {
                tok: Tok::Str(parts),
                span: Span::new(start, end),
            });
            pos = end;
            continue;
        }
        if is_ident_start(c) {
            let mut end = scan_ident(b, pos);
            // Module-qualified names: `mod::name`, repeatable.
            while b.get(end) == Some(&b':')
                && b.get(end + 1) == Some(&b':')
                && b.get(end + 2).is_some_and(|&x| is_ident_start(x))
            {
                end = scan_ident(b, end + 2);
            }
            let word = &src[pos..end];
            let tok = match Kw::from_word(word) {
                Some(k) => Tok::Keyword(k),
                None => Tok::Ident(word.to_owned()),
            };
            out.push(Token {
                tok,
                span: Span::new(start, end),
            });
            pos = end;
            continue;
        }
        if c == b'$' && b.get(pos + 1).is_some_and(|&x| is_ident_start(x)) {
            let mut end = scan_ident(b, pos + 1);
            while b.get(end) == Some(&b':')
                && b.get(end + 1) == Some(&b':')
                && b.get(end + 2).is_some_and(|&x| is_ident_start(x))
            {
                end = scan_ident(b, end + 2);
            }
            let name = &src[pos + 1..end];
            let tok = if name == "__loc__" {
                Tok::Loc
            } else {
                Tok::Binding(name.to_owned())
            };
            out.push(Token {
                tok,
                span: Span::new(start, end),
            });
            pos = end;
            continue;
        }
        if c == b'@' && b.get(pos + 1).is_some_and(|&x| is_ident_char(x)) {
            let end = scan_ident(b, pos + 1);
            out.push(Token {
                tok: Tok::Format(src[pos + 1..end].to_owned()),
                span: Span::new(start, end),
            });
            pos = end;
            continue;
        }
        if c.is_ascii_digit() || (c == b'.' && b.get(pos + 1).is_some_and(u8::is_ascii_digit)) {
            let end = scan_number(b, pos);
            out.push(Token {
                tok: Tok::Number,
                span: Span::new(start, end),
            });
            pos = end;
            continue;
        }
        if c == b'.' && b.get(pos + 1).is_some_and(|&x| is_ident_start(x)) {
            let end = scan_ident(b, pos + 1);
            out.push(Token {
                tok: Tok::Field(src[pos + 1..end].to_owned()),
                span: Span::new(start, end),
            });
            pos = end;
            continue;
        }
        let Some(op) = OPS.iter().copied().find(|op| src[pos..].starts_with(op)) else {
            return Err(LexError(pos));
        };
        if interp {
            match op {
                "(" => depth += 1,
                ")" if depth == 0 => return Ok((out, pos + 1)),
                ")" => depth -= 1,
                _ => {}
            }
        }
        out.push(Token {
            tok: Tok::Op(op),
            span: Span::new(start, pos + op.len()),
        });
        pos += op.len();
    }
}

/// Skip a `#` comment. A backslash escapes the next backslash or newline, so a
/// comment line ending in `\` continues onto the next line.
fn skip_comment(b: &[u8], mut pos: usize) -> usize {
    pos += 1;
    while let Some(&c) = b.get(pos) {
        match c {
            b'\\' => match b.get(pos + 1) {
                Some(b'\\' | b'\n') => pos += 2,
                Some(b'\r') if b.get(pos + 2) == Some(&b'\n') => pos += 3,
                _ => pos += 1,
            },
            b'\n' => return pos + 1,
            _ => pos += 1,
        }
    }
    pos
}

fn scan_ident(b: &[u8], mut pos: usize) -> usize {
    while b.get(pos).is_some_and(|&x| is_ident_char(x)) {
        pos += 1;
    }
    pos
}

/// `([0-9]+(\.[0-9]*)?|\.[0-9]+)([eE][+-]?[0-9]+)?`
fn scan_number(b: &[u8], mut pos: usize) -> usize {
    let digits = |p: &mut usize| {
        while b.get(*p).is_some_and(u8::is_ascii_digit) {
            *p += 1;
        }
    };
    if b[pos] == b'.' {
        pos += 1;
        digits(&mut pos);
    } else {
        digits(&mut pos);
        if b.get(pos) == Some(&b'.') {
            pos += 1;
            digits(&mut pos);
        }
    }
    if matches!(b.get(pos), Some(b'e' | b'E')) {
        let mut p = pos + 1;
        if matches!(b.get(p), Some(b'+' | b'-')) {
            p += 1;
        }
        if b.get(p).is_some_and(u8::is_ascii_digit) {
            digits(&mut p);
            pos = p;
        }
    }
    pos
}

/// Lex a string body starting just after the opening quote. Returns the parts
/// and the position just past the closing quote.
fn lex_string(src: &str, mut pos: usize) -> Result<(Vec<StrPart>, usize), LexError> {
    let b = src.as_bytes();
    let mut parts = Vec::new();
    let mut text_start = pos;
    loop {
        match b.get(pos) {
            None => return Err(LexError(pos)),
            Some(b'"') => {
                if pos > text_start {
                    parts.push(StrPart::Text(Span::new(text_start, pos)));
                }
                return Ok((parts, pos + 1));
            }
            Some(b'\\') if b.get(pos + 1) == Some(&b'(') => {
                if pos > text_start {
                    parts.push(StrPart::Text(Span::new(text_start, pos)));
                }
                let (toks, end) = lex_from(src, pos + 2, true)?;
                parts.push(StrPart::Interp(toks, Span::new(pos, end)));
                pos = end;
                text_start = pos;
            }
            Some(b'\\') => pos += 2,
            Some(_) => pos += 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Kw, Tok, lex};

    fn kinds(src: &str) -> Vec<String> {
        lex(src)
            .expect("lexes")
            .into_iter()
            .map(|t| match t.tok {
                Tok::Ident(s) => format!("I:{s}"),
                Tok::Field(s) => format!("F:{s}"),
                Tok::Binding(s) => format!("B:{s}"),
                Tok::Loc => "LOC".to_owned(),
                Tok::Format(s) => format!("@{s}"),
                Tok::Number => format!("N:{}", &src[t.span.start..t.span.end]),
                Tok::Str(p) => format!("S{}", p.len()),
                Tok::Keyword(k) => format!("K:{}", k.as_str()),
                Tok::Op(o) => o.to_owned(),
                Tok::Eof => "EOF".to_owned(),
            })
            .collect()
    }

    #[test]
    fn keywords_need_the_whole_word() {
        assert_eq!(kinds("if iffy"), ["K:if", "I:iffy", "EOF"]);
        assert_eq!(kinds(".if"), ["F:if", "EOF"]);
    }

    #[test]
    fn numbers_and_fields_are_told_apart_by_their_second_byte() {
        assert_eq!(
            kinds(".5 .e5 1.5e3 1."),
            ["N:.5", "F:e5", "N:1.5e3", "N:1.", "EOF"]
        );
    }

    #[test]
    fn longest_match_makes_alternation_a_single_token() {
        // jq rejects `.a?//1` because of this; reproducing it is the point.
        assert_eq!(kinds(".a?//1"), ["F:a", "?//", "N:1", "EOF"]);
        assert_eq!(kinds("a //= b"), ["I:a", "//=", "I:b", "EOF"]);
    }

    #[test]
    fn module_qualified_names_and_bindings() {
        assert_eq!(
            kinds("m::f $m::x $__loc__ $__loc__x"),
            ["I:m::f", "B:m::x", "LOC", "B:__loc__x", "EOF"]
        );
    }

    #[test]
    fn comments_continue_after_a_trailing_backslash() {
        assert_eq!(kinds("1 # c \\\n still comment\n2"), ["N:1", "N:2", "EOF"]);
        assert_eq!(kinds("1 # c\n2"), ["N:1", "N:2", "EOF"]);
    }

    #[test]
    fn interpolations_nest() {
        let toks = lex(r#""a\("b\(1)")c""#).expect("lexes");
        let Tok::Str(parts) = &toks[0].tok else {
            panic!("string expected")
        };
        assert_eq!(parts.len(), 3, "text, interpolation, text");
        assert_eq!(toks.len(), 2, "one string token and EOF");
    }

    #[test]
    fn parens_inside_an_interpolation_do_not_end_it() {
        let toks = lex(r#""\((1 + (2)))x""#).expect("lexes");
        let Tok::Str(parts) = &toks[0].tok else {
            panic!("string expected")
        };
        assert_eq!(parts.len(), 2);
    }

    #[test]
    fn unterminated_strings_are_errors() {
        lex("\"abc").unwrap_err();
        lex("\"\\(1\"").unwrap_err();
    }

    #[test]
    fn keyword_enum_round_trips() {
        assert_eq!(Kw::Foreach.as_str(), "foreach");
    }
}
