// SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
// SPDX-License-Identifier: GPL-3.0-or-later

//! jq's syntax errors, found and worded as jq 1.8.1 finds and words them
//! (PLAN P-069).
//!
//! jq's parser is bison's: its messages (`syntax error, unexpected IDENT,
//! expecting end of file`) come from the LALR automaton, and which tokens
//! it lists depends on the state the automaton reached. So this module runs
//! jq's own automaton: [`super::jq_tables`] are the tables bison generated
//! for jq 1.8.1, and [`syntax_errors`] is bison 3.8's driver over them, fed
//! by a port of jq's lexer (`src/lexer.l`). Nothing is built from the parse;
//! the driver only decides where jq's grammar fails, recovers as bison
//! recovers, and keeps the messages jq's error rules add (`Possibly
//! unterminated 'if' statement`, …) and its lexer's (`Invalid escape …`).
//!
//! Locations are jq's: the byte span of the token, where every lexer match,
//! whitespace and comments included, moves the location, so an error at
//! the end of the program points at whatever was matched last.

use super::Span;
use super::check::CompileError;
use super::jq_tables::{self as t, tok};

/// bison's `YYMAXDEPTH`: past it the parse fails as `memory exhausted`.
const MAX_DEPTH: usize = 10_000;

/// The symbol number of bison's `error` token.
const SYM_ERROR: i32 = 1;

/// The syntax errors jq 1.8.1 reports for `src`, in its order; empty when
/// jq's grammar accepts it. Spans are relative to `src`.
///
/// A program the grammar accepts with no main expression (nothing but
/// definitions, a header, comments, or nothing at all) is jq's
/// `Top-level program not given`, which `load_program` raises without a
/// location.
pub fn syntax_errors(src: &str) -> Vec<CompileError> {
    let mut driver = Driver {
        lexer: Lexer::new(src.as_bytes()),
        errors: Vec::new(),
        main: true,
    };
    driver.run();
    if driver.errors.is_empty() && !driver.main {
        driver.error(
            "Top-level program not given (try \".\")".to_owned(),
            CompileError::UNLOCATED,
        );
    }
    driver.errors
}

// --- The lexer ---------------------------------------------------------------

/// flex's start conditions in jq's lexer: the first five are inclusive (the
/// unprefixed rules apply), the last two exclusive.
#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Initial,
    Paren,
    Bracket,
    Brace,
    Interp,
    String,
    Comment,
}

struct Token {
    code: i32,
    span: Span,
    /// The message jq's `yylex` raises for an invalid literal.
    error: Option<String>,
}

struct Lexer<'a> {
    b: &'a [u8],
    pos: usize,
    states: Vec<State>,
    /// The location of the last match, as `YY_USER_ACTION` leaves it.
    last: Span,
}

/// The fixed spellings, in `lexer.l`'s order, which breaks ties of length.
const FIXED: [(&str, i32); 33] = [
    ("!=", tok::NEQ),
    ("==", tok::EQ),
    ("as", tok::AS),
    ("import", tok::IMPORT),
    ("include", tok::INCLUDE),
    ("module", tok::MODULE),
    ("def", tok::DEF),
    ("if", tok::IF),
    ("then", tok::THEN),
    ("else", tok::ELSE),
    ("elif", tok::ELSE_IF),
    ("and", tok::AND),
    ("or", tok::OR),
    ("end", tok::END),
    ("reduce", tok::REDUCE),
    ("foreach", tok::FOREACH),
    ("//", tok::DEFINEDOR),
    ("try", tok::TRY),
    ("catch", tok::CATCH),
    ("label", tok::LABEL),
    ("break", tok::BREAK),
    ("$__loc__", tok::LOC),
    ("|=", tok::SETPIPE),
    ("+=", tok::SETPLUS),
    ("-=", tok::SETMINUS),
    ("*=", tok::SETMULT),
    ("/=", tok::SETDIV),
    ("%=", tok::SETMOD),
    ("//=", tok::SETDEFINEDOR),
    ("<=", tok::LESSEQ),
    (">=", tok::GREATEREQ),
    ("..", tok::REC),
    ("?//", tok::ALTERNATION),
];

/// What a rule in the unprefixed set does with its match.
#[derive(Clone, Copy)]
enum Act {
    Comment,
    Return(i32),
    Enter(u8),
    Exit(u8),
    StringStart,
    Skip,
}

const fn ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_'
}

const fn ident_rest(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

impl<'a> Lexer<'a> {
    const fn new(b: &'a [u8]) -> Self {
        Self {
            b,
            pos: 0,
            states: Vec::new(),
            last: Span { start: 0, end: 0 },
        }
    }

    fn state(&self) -> State {
        self.states.last().copied().unwrap_or(State::Initial)
    }

    fn at(&self, i: usize) -> Option<u8> {
        self.b.get(i).copied()
    }

    /// `[a-zA-Z_][a-zA-Z_0-9]*` at `i`: its end.
    fn segment(&self, i: usize) -> Option<usize> {
        if !self.at(i).is_some_and(ident_start) {
            return None;
        }
        let mut e = i + 1;
        while self.at(e).is_some_and(ident_rest) {
            e += 1;
        }
        Some(e)
    }

    /// `([a-zA-Z_][a-zA-Z_0-9]*::)*[a-zA-Z_][a-zA-Z_0-9]*` at `i`.
    fn ident(&self, i: usize) -> Option<usize> {
        let mut e = self.segment(i)?;
        while self.at(e) == Some(b':') && self.at(e + 1) == Some(b':') {
            match self.segment(e + 2) {
                Some(next) => e = next,
                None => break,
            }
        }
        Some(e)
    }

    /// `([0-9]+(\.[0-9]*)?|\.[0-9]+)([eE][+-]?[0-9]+)?` at `i`.
    fn number(&self, i: usize) -> Option<usize> {
        let digits = |mut e: usize| {
            while self.at(e).is_some_and(|c| c.is_ascii_digit()) {
                e += 1;
            }
            e
        };
        let mut e = digits(i);
        if e > i {
            if self.at(e) == Some(b'.') {
                e = digits(e + 1);
            }
        } else if self.at(i) == Some(b'.') && self.at(i + 1).is_some_and(|c| c.is_ascii_digit()) {
            e = digits(i + 1);
        } else {
            return None;
        }
        if matches!(self.at(e), Some(b'e' | b'E')) {
            let s = if matches!(self.at(e + 1), Some(b'+' | b'-')) {
                e + 2
            } else {
                e + 1
            };
            let d = digits(s);
            if d > s {
                e = d;
            }
        }
        Some(e)
    }

    /// The longest match of the unprefixed rules at `self.pos`, the earliest
    /// rule among equals: its length and action.
    fn longest(&self) -> (usize, Act) {
        let i = self.pos;
        let rest = &self.b[i..];
        let mut best: (usize, Act) = (0, Act::Skip);
        let mut offer = |len: usize, act: Act| {
            if len > best.0 {
                best = (len, act);
            }
        };
        let c = rest[0];
        if c == b'#' {
            offer(1, Act::Comment);
        }
        // The fixed spellings that are not words; a keyword is an
        // identifier of its own spelling, taken below.
        if b"!=/$|+-*%<>.?".contains(&c) {
            for (s, code) in FIXED {
                if s.as_bytes()[0] == c && rest.starts_with(s.as_bytes()) {
                    offer(s.len(), Act::Return(code));
                }
            }
        }
        if b".?=;,:|+-*/%$<>".contains(&c) {
            offer(1, Act::Return(i32::from(c)));
        }
        if b"[{(".contains(&c) {
            offer(1, Act::Enter(c));
        }
        if b"]})".contains(&c) {
            offer(1, Act::Exit(c));
        }
        if c == b'@' {
            let mut e = i + 1;
            while self.at(e).is_some_and(ident_rest) {
                e += 1;
            }
            if e > i + 1 {
                offer(e - i, Act::Return(tok::FORMAT));
            }
        }
        if let Some(e) = self.number(i) {
            offer(e - i, Act::Return(tok::LITERAL));
        }
        if c == b'"' {
            offer(1, Act::StringStart);
        }
        if let Some(e) = self.ident(i) {
            // A keyword's rule comes before IDENT's, so it wins a tie.
            let word = &rest[..e - i];
            let keyword = FIXED
                .iter()
                .find(|(s, _)| s.as_bytes() == word)
                .map(|&(_, code)| code);
            offer(e - i, Act::Return(keyword.unwrap_or(tok::IDENT)));
        }
        if c == b'.'
            && let Some(e) = self.segment(i + 1)
        {
            offer(e - i, Act::Return(tok::FIELD));
        }
        if c == b'$'
            && let Some(e) = self.ident(i + 1)
        {
            offer(e - i, Act::Return(tok::BINDING));
        }
        if b" \r\n\t".contains(&c) {
            let n = rest.iter().take_while(|c| b" \r\n\t".contains(c)).count();
            offer(n, Act::Skip);
        }
        if c != b'\n' {
            offer(1, Act::Return(tok::INVALID_CHARACTER));
        }
        best
    }

    /// Consume `len` bytes as one match, moving the location as
    /// `YY_USER_ACTION` does.
    fn take(&mut self, len: usize) -> Span {
        let span = Span::new(self.pos, self.pos + len);
        self.pos += len;
        self.last = span;
        span
    }

    const fn token(code: i32, span: Span) -> Token {
        Token {
            code,
            span,
            error: None,
        }
    }

    fn next(&mut self) -> Token {
        loop {
            if self.pos >= self.b.len() {
                // `<IN_COMMENT><<EOF>>` pops; no rule moves the location.
                if self.state() == State::Comment {
                    self.states.pop();
                }
                return Self::token(0, self.last);
            }
            match self.state() {
                State::Comment => self.comment(),
                State::String => return self.string(),
                _ => {
                    let (len, act) = self.longest();
                    let span = self.take(len);
                    match act {
                        Act::Comment => self.states.push(State::Comment),
                        Act::Skip => {}
                        Act::Return(code) => return Self::token(code, span),
                        Act::StringStart => {
                            self.states.push(State::String);
                            return Self::token(tok::QQSTRING_START, span);
                        }
                        Act::Enter(c) => {
                            self.states.push(match c {
                                b'(' => State::Paren,
                                b'[' => State::Bracket,
                                _ => State::Brace,
                            });
                            return Self::token(i32::from(c), span);
                        }
                        Act::Exit(c) => return Self::token(self.exit(c), span),
                    }
                }
            }
        }
    }

    /// jq's `try_exit`: a closer pops its own opener; anything else is
    /// `INVALID_CHARACTER`, and pops nothing.
    fn exit(&mut self, c: u8) -> i32 {
        let (want, code) = match self.state() {
            State::Paren => (b')', i32::from(b')')),
            State::Bracket => (b']', i32::from(b']')),
            State::Brace => (b'}', i32::from(b'}')),
            State::Interp => (b')', tok::QQSTRING_INTERP_END),
            _ => return tok::INVALID_CHARACTER,
        };
        if c == want {
            self.states.pop();
            code
        } else {
            tok::INVALID_CHARACTER
        }
    }

    /// `<IN_COMMENT>`: `\\(\\|\r?\n)|.` is skipped, `\r?\n` ends it.
    fn comment(&mut self) {
        let i = self.pos;
        let (len, ends) = match (self.at(i), self.at(i + 1), self.at(i + 2)) {
            (Some(b'\\'), Some(b'\\' | b'\n'), _) => (2, false),
            (Some(b'\\'), Some(b'\r'), Some(b'\n')) => (3, false),
            (Some(b'\r'), Some(b'\n'), _) => (2, true),
            (Some(b'\n'), _, _) => (1, true),
            _ => (1, false),
        };
        self.take(len);
        if ends {
            self.states.pop();
        }
    }

    /// `<IN_QQSTRING>`.
    fn string(&mut self) -> Token {
        let i = self.pos;
        match (self.at(i), self.at(i + 1)) {
            (Some(b'\\'), Some(b'(')) => {
                let span = self.take(2);
                self.states.push(State::Interp);
                Self::token(tok::QQSTRING_INTERP_START, span)
            }
            (Some(b'"'), _) => {
                let span = self.take(1);
                self.states.pop();
                Self::token(tok::QQSTRING_END, span)
            }
            (Some(b'\\'), Some(_)) => {
                // `(\\[^u(]|\\u[a-zA-Z0-9]{0,4})+`, handed to jq's JSON parser.
                let mut e = i;
                loop {
                    match (self.at(e), self.at(e + 1)) {
                        (Some(b'\\'), Some(b'u')) => {
                            e += 2;
                            let mut n = 0;
                            while n < 4 && self.at(e).is_some_and(|c| c.is_ascii_alphanumeric()) {
                                e += 1;
                                n += 1;
                            }
                        }
                        (Some(b'\\'), Some(c)) if c != b'(' => e += 2,
                        _ => break,
                    }
                }
                let span = self.take(e - i);
                Token {
                    code: tok::QQSTRING_TEXT,
                    span,
                    error: escapes(&self.b[span.start..span.end]),
                }
            }
            (Some(b'\\'), None) => {
                let span = self.take(1);
                Self::token(tok::INVALID_CHARACTER, span)
            }
            _ => {
                let n = self.b[i..]
                    .iter()
                    .take_while(|c| !matches!(c, b'\\' | b'"'))
                    .count();
                let span = self.take(n);
                Self::token(tok::QQSTRING_TEXT, span)
            }
        }
    }
}

/// jq's JSON parser on `"<run>"`, for a run of escapes (`found_string` in
/// `src/jv_parse.c`): its message, if it rejects them.
fn escapes(run: &[u8]) -> Option<String> {
    let hex4 = |s: &[u8]| -> Option<u32> {
        let d = s.get(..4)?;
        let text = std::str::from_utf8(d).ok()?;
        if d.iter().all(u8::is_ascii_hexdigit) {
            u32::from_str_radix(text, 16).ok()
        } else {
            None
        }
    };
    let mut i = 0;
    let failure = loop {
        if i >= run.len() {
            return None;
        }
        // A run holds escapes only: a backslash, then what it escapes.
        let Some(&c) = run.get(i + 1) else {
            break "Expected escape character at end of string";
        };
        i += 2;
        match c {
            b'\\' | b'"' | b'/' | b'b' | b'f' | b't' | b'n' | b'r' => {}
            b'u' => {
                if i + 4 > run.len() {
                    break "Invalid \\uXXXX escape";
                }
                let Some(cp) = hex4(&run[i..]) else {
                    break "Invalid characters in \\uXXXX escape";
                };
                i += 4;
                if (0xD800..=0xDBFF).contains(&cp) {
                    if i + 6 > run.len() || run[i] != b'\\' || run[i + 1] != b'u' {
                        break "Invalid \\uXXXX\\uXXXX surrogate pair escape";
                    }
                    if !hex4(&run[i + 2..]).is_some_and(|low| (0xDC00..=0xDFFF).contains(&low)) {
                        break "Invalid \\uXXXX\\uXXXX surrogate pair escape";
                    }
                    i += 6;
                }
            }
            _ => break "Invalid escape",
        }
    };
    // The parser has read the whole quoted text when it fails; its line and
    // column count as jq's do, a newline starting column 0 again.
    let snippet = format!("\"{}\"", String::from_utf8_lossy(run));
    let line = 1 + snippet.matches('\n').count();
    let column = snippet.len() - snippet.rfind('\n').map_or(0, |n| n + 1);
    Some(format!(
        "{failure} at line {line}, column {column} (while parsing '{snippet}')"
    ))
}

// --- The driver --------------------------------------------------------------

/// The messages of jq's error rules (`FAIL` in `src/parser.y`), by rule
/// number in its tables, and whether the location is the rule's own (`@$`)
/// or its first symbol's (`@1`).
fn rule_message(rule: usize) -> Option<(&'static str, bool)> {
    const FIELD: &str = "try .[\"field\"] instead of .field for unusually named fields";
    const KEY: &str = "May need parentheses around object key expression";
    Some(match rule {
        63 => ("break requires a label to break to", false),
        72 | 73 => (FIELD, false),
        101 => ("Possibly unterminated 'if' statement", false),
        103 => ("Possibly unterminated 'try' statement", false),
        134 => (KEY, false),
        166 => (KEY, true),
        _ => return None,
    })
}

/// bison's `yytnamerr`: a name in double quotes is shown without them.
fn symbol_name(sym: i32) -> String {
    let name = usize::try_from(sym)
        .ok()
        .and_then(|i| t::YYTNAME.get(i))
        .copied()
        .unwrap_or("\"invalid token\"");
    match name.strip_prefix('"').and_then(|n| n.strip_suffix('"')) {
        Some(inner) if !inner.contains(['\'', ',', '\\']) => inner.to_owned(),
        _ => name.to_owned(),
    }
}

fn table(i: i32) -> i32 {
    usize::try_from(i)
        .ok()
        .and_then(|i| t::YYTABLE.get(i))
        .map_or(0, |&v| i32::from(v))
}

fn check(i: i32) -> i32 {
    usize::try_from(i)
        .ok()
        .and_then(|i| t::YYCHECK.get(i))
        .map_or(-1, |&v| i32::from(v))
}

fn pact(state: i32) -> i32 {
    usize::try_from(state)
        .ok()
        .and_then(|i| t::YYPACT.get(i))
        .map_or(t::YYPACT_NINF, |&v| i32::from(v))
}

/// The symbol bison reads for a token code (`YYTRANSLATE`).
fn translate(code: i32) -> i32 {
    if code <= 0 {
        return 0;
    }
    usize::try_from(code)
        .ok()
        .and_then(|i| t::YYTRANSLATE.get(i))
        .map_or(2, |&v| i32::from(v))
}

/// bison's `yypcontext_expected_tokens`: the tokens `state` acts on, or
/// none if there are more than four.
fn expected(state: i32) -> Vec<i32> {
    let mut out = Vec::new();
    let n = pact(state);
    if n == t::YYPACT_NINF {
        return out;
    }
    let begin = if n < 0 { -n } else { 0 };
    let end = (t::YYLAST - n + 1).min(t::YYNTOKENS);
    for x in begin..end {
        if check(x + n) == x && x != SYM_ERROR && table(x + n) != t::YYTABLE_NINF {
            if out.len() == 4 {
                return Vec::new();
            }
            out.push(x);
        }
    }
    out
}

/// bison's verbose message for an error on `token` in `state`.
fn syntax_message(state: i32, token: Option<i32>) -> String {
    let Some(token) = token else {
        return "syntax error".to_owned();
    };
    let mut msg = format!("syntax error, unexpected {}", symbol_name(token));
    for (i, x) in expected(state).into_iter().enumerate() {
        msg.push_str(if i == 0 { ", expecting " } else { " or " });
        msg.push_str(&symbol_name(x));
    }
    msg
}

struct Driver<'a> {
    lexer: Lexer<'a>,
    errors: Vec<CompileError>,
    /// Whether the program has a main expression: `TopLevel` was reduced
    /// by rule 2 (`Module Imports Query`), not 3 (`Module Imports FuncDefs`).
    main: bool,
}

impl Driver<'_> {
    fn error(&mut self, message: String, span: Span) {
        self.errors.push(CompileError {
            message,
            span,
            more: Vec::new(),
        });
    }

    /// bison 3.8's `yyparse` on jq's tables, without semantic values.
    fn run(&mut self) {
        let mut states: Vec<i32> = Vec::from([0]);
        let mut locs: Vec<Span> = Vec::from([Span::new(0, 0)]);
        // The lookahead's token code, `None` for `YYEMPTY`, and `yylloc`.
        let mut lookahead: Option<i32> = None;
        let mut lloc = Span::new(0, 0);
        let mut errstatus = 0;
        // Every step shifts, reduces or discards; this bounds a bug, not jq.
        let mut budget = 64 * (self.lexer.b.len() + 64) * 64;
        loop {
            if budget == 0 {
                return;
            }
            budget -= 1;
            let state = *states.last().unwrap_or(&0);
            if state == t::YYFINAL {
                return;
            }
            if states.len() >= MAX_DEPTH {
                self.error("memory exhausted".to_owned(), lloc);
                return;
            }
            // `yybackup`: shift, reduce, or fail.
            let mut action: Option<i32> = None; // > 0 shift, < 0 reduce
            let n = pact(state);
            if n != t::YYPACT_NINF {
                let code = if let Some(code) = lookahead {
                    code
                } else {
                    let tok = self.lexer.next();
                    lloc = tok.span;
                    if let Some(m) = tok.error {
                        self.error(m, tok.span);
                    }
                    lookahead = Some(tok.code);
                    tok.code
                };
                let sym = translate(code);
                let i = n + sym;
                if (0..=t::YYLAST).contains(&i) && check(i) == sym {
                    let v = table(i);
                    action = Some(if v == t::YYTABLE_NINF { 0 } else { v });
                }
            }
            let action = action.unwrap_or_else(|| {
                let r = usize::try_from(state)
                    .ok()
                    .and_then(|s| t::YYDEFACT.get(s))
                    .map_or(0, |&r| i32::from(r));
                -r
            });
            match action.cmp(&0) {
                std::cmp::Ordering::Greater => {
                    errstatus = std::cmp::max(errstatus - 1, 0);
                    states.push(action);
                    locs.push(lloc);
                    lookahead = None;
                }
                std::cmp::Ordering::Less => self.reduce(-action, &mut states, &mut locs),
                std::cmp::Ordering::Equal => {
                    // `yyerrlab`
                    if errstatus == 0 {
                        let token = lookahead.map(translate);
                        self.error(syntax_message(state, token), lloc);
                    }
                    let mut range_start = lloc;
                    if errstatus == 3 {
                        match lookahead {
                            Some(code) if code <= 0 => return,
                            Some(_) => lookahead = None,
                            None => {}
                        }
                    }
                    // `yyerrlab1`: pop to a state that shifts `error`.
                    errstatus = 3;
                    let target = loop {
                        let top = *states.last().unwrap_or(&0);
                        let n = pact(top);
                        if n != t::YYPACT_NINF {
                            let i = n + SYM_ERROR;
                            if (0..=t::YYLAST).contains(&i) && check(i) == SYM_ERROR && table(i) > 0
                            {
                                break table(i);
                            }
                        }
                        if states.len() == 1 {
                            return;
                        }
                        range_start = *locs.last().unwrap_or(&range_start);
                        states.pop();
                        locs.pop();
                    };
                    states.push(target);
                    locs.push(Span::new(range_start.start, lloc.end));
                }
            }
        }
    }

    /// `yyreduce`, with jq's `YYLLOC_DEFAULT` and its error rules' `FAIL`.
    fn reduce(&mut self, rule: i32, states: &mut Vec<i32>, locs: &mut Vec<Span>) {
        let r = usize::try_from(rule).unwrap_or(0);
        let len = t::YYR2.get(r).map_or(0, |&l| usize::from(l));
        let len = len.min(states.len() - 1);
        let top = locs.len() - 1;
        let loc = if len > 0 {
            Span::new(locs[top + 1 - len].start, locs[top].end)
        } else {
            Span::new(locs[top].end, locs[top].end)
        };
        match r {
            2 => self.main = true,
            3 => self.main = false,
            _ => {}
        }
        if let Some((message, first)) = rule_message(r) {
            let at = if first { locs[top + 1 - len] } else { loc };
            self.error(message.to_owned(), at);
        }
        states.truncate(states.len() - len);
        locs.truncate(locs.len() - len);
        locs.push(loc);
        let lhs = t::YYR1.get(r).map_or(0, |&s| i32::from(s)) - t::YYNTOKENS;
        let below = *states.last().unwrap_or(&0);
        let li = usize::try_from(lhs).unwrap_or(0);
        let i = t::YYPGOTO.get(li).map_or(0, |&v| i32::from(v)) + below;
        let next = if (0..=t::YYLAST).contains(&i) && check(i) == below {
            table(i)
        } else {
            t::YYDEFGOTO.get(li).map_or(0, |&v| i32::from(v))
        };
        states.push(next);
    }
}
