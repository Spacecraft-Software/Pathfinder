// SPDX-FileCopyrightText: 2002-2024 K.Kosako
// SPDX-FileCopyrightText: 2026 Mohamed Hammad <Mohamed.Hammad@SpacecraftSoftware.org>
// SPDX-License-Identifier: BSD-2-Clause AND (GPL-3.0-or-later OR MIT)

//! Oniguruma's errors, worded as `onig_error_code_to_str` words them
//! (`src/regerror.c`).

use std::fmt;

/// The errors this syntax and encoding can raise (`ONIGERR_*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorKind {
    ParseDepthLimitOver,
    RetryLimitInMatchOver,
    EmptyCharClass,
    PrematureEndOfCharClass,
    EndPatternAtEscape,
    EndPatternAtControl,
    CharClassValueAtEndOfRange,
    UnmatchedRangeSpecifierInCharClass,
    TargetOfRepeatOperatorNotSpecified,
    TargetOfRepeatOperatorInvalid,
    UnmatchedCloseParenthesis,
    EndPatternWithUnmatchedParenthesis,
    EndPatternInGroup,
    UndefinedGroupOption,
    InvalidPosixBracketType,
    InvalidLookBehindPattern,
    TooBigNumber,
    TooBigNumberForRepeatRange,
    UpperSmallerThanLowerInRepeatRange,
    EmptyRangeInCharClass,
    TooShortMultiByteString,
    InvalidBackref,
    TooLongWideCharValue,
    InvalidCodePointValue,
    EmptyGroupName,
    InvalidGroupName,
    InvalidCharInGroupName,
    UndefinedNameReference,
    UndefinedGroupReference,
    InvalidCharPropertyName,
    InvalidIfElseSyntax,
    /// A status Oniguruma has no message for, which it words "undefined
    /// error code" (a lookbehind whose every item reduces away).
    UndefinedErrorCode,
    /// A construct Oniguruma has that this engine does not (TODO T-044):
    /// subexpression calls, the absent operator, callouts.
    Unsupported,
}

impl ErrorKind {
    /// Oniguruma's message, `%n` where the name goes.
    const fn format(self) -> &'static str {
        match self {
            Self::ParseDepthLimitOver => "parse depth limit over",
            Self::RetryLimitInMatchOver => "retry-limit-in-match over",
            Self::EmptyCharClass => "empty char-class",
            Self::PrematureEndOfCharClass => "premature end of char-class",
            Self::EndPatternAtEscape => "end pattern at escape",
            Self::EndPatternAtControl => "end pattern at control",
            Self::CharClassValueAtEndOfRange => "char-class value at end of range",
            Self::UnmatchedRangeSpecifierInCharClass => "unmatched range specifier in char-class",
            Self::TargetOfRepeatOperatorNotSpecified => {
                "target of repeat operator is not specified"
            }
            Self::TargetOfRepeatOperatorInvalid => "target of repeat operator is invalid",
            Self::UnmatchedCloseParenthesis => "unmatched close parenthesis",
            Self::EndPatternWithUnmatchedParenthesis => "end pattern with unmatched parenthesis",
            Self::EndPatternInGroup => "end pattern in group",
            Self::UndefinedGroupOption => "undefined group option",
            Self::InvalidPosixBracketType => "invalid POSIX bracket type",
            Self::InvalidLookBehindPattern => "invalid pattern in look-behind",
            Self::TooBigNumber => "too big number",
            Self::TooBigNumberForRepeatRange => "too big number for repeat range",
            Self::UpperSmallerThanLowerInRepeatRange => {
                "upper is smaller than lower in repeat range"
            }
            Self::EmptyRangeInCharClass => "empty range in char class",
            Self::TooShortMultiByteString => "too short multibyte code string",
            Self::InvalidBackref => "invalid backref number/name",
            Self::TooLongWideCharValue => "too long wide-char value",
            Self::InvalidCodePointValue => "invalid code point value",
            Self::EmptyGroupName => "group name is empty",
            Self::InvalidGroupName => "invalid group name <%n>",
            Self::InvalidCharInGroupName => "invalid char in group name <%n>",
            Self::UndefinedNameReference => "undefined name <%n> reference",
            Self::UndefinedGroupReference => "undefined group <%n> reference",
            Self::InvalidCharPropertyName => "invalid character property name {%n}",
            Self::InvalidIfElseSyntax => "invalid if-else syntax",
            Self::UndefinedErrorCode => "undefined error code",
            Self::Unsupported => {
                "subexpression calls, absent groups and callouts are not supported"
            }
        }
    }
}

/// A pattern Oniguruma rejects, or a match it gives up on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    kind: ErrorKind,
    /// The name a message quotes (`einfo->par`).
    name: Vec<u8>,
}

/// The bytes of a name a message quotes (`MAX_ERROR_PAR_LEN - 3`).
const NAME_MAX: usize = 27;

impl Error {
    pub(crate) const fn new(kind: ErrorKind) -> Self {
        Self {
            kind,
            name: Vec::new(),
        }
    }

    pub(crate) fn with_name(kind: ErrorKind, name: &[u8]) -> Self {
        Self {
            kind,
            name: name.to_vec(),
        }
    }

    /// What went wrong.
    #[must_use]
    pub const fn kind(&self) -> ErrorKind {
        self.kind
    }

    /// The message as Oniguruma words it, as bytes: a quoted name is cut
    /// after 27 bytes and marked `...`, which can split a character.
    #[must_use]
    pub fn message(&self) -> Vec<u8> {
        let mut out = Vec::new();
        let fmt = self.kind.format().as_bytes();
        let mut i = 0;
        while i < fmt.len() {
            if fmt[i] == b'%' && fmt.get(i + 1) == Some(&b'n') {
                let n = self.name.len().min(NAME_MAX);
                out.extend_from_slice(&self.name[..n]);
                if self.name.len() > NAME_MAX {
                    out.extend_from_slice(b"...");
                }
                i += 2;
            } else {
                out.push(fmt[i]);
                i += 1;
            }
        }
        out
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&String::from_utf8_lossy(&self.message()))
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::{Error, ErrorKind};

    #[test]
    fn a_long_name_is_cut_as_oniguruma_cuts_it() {
        let e = Error::with_name(
            ErrorKind::InvalidCharPropertyName,
            b"nosuchpropertyreallylongname",
        );
        assert_eq!(
            e.to_string(),
            "invalid character property name {nosuchpropertyreallylongnam...}"
        );
        assert_eq!(
            Error::new(ErrorKind::EmptyGroupName).to_string(),
            "group name is empty"
        );
    }
}
