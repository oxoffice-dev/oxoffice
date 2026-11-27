//! The tokens of a formula: every character belongs to one token, whitespace
//! included, so the tokens joined give the formula back.

use std::ops::Range;

use crate::SyntaxError;
use crate::reference::{is_name_char, is_name_start, target_at};

/// What a token is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TokenKind {
    /// Spaces and line breaks. Between two references, a space is the
    /// intersection operator: the parser says which.
    Whitespace,
    /// `12`, `1.5`, `.5`, `1E+10`.
    Number,
    /// `"a ""quoted"" text"`, quotes included.
    Text,
    /// `TRUE` or `FALSE`, not followed by `(`.
    Boolean,
    /// `#N/A`, `#DIV/0!`, `#REF!`...
    Error,
    /// A cell, an area, whole columns or rows, or `#REF!`, with its sheet
    /// or workbook prefix: `B3`, `$A$1:$C$10`, `A:C`, `2:5`,
    /// `'Q1 data'!B3`, `Sheet1:Sheet3!A1`, `[1]Sheet1!A1`, `Sheet1!#REF!`.
    Reference,
    /// A defined name, a LET or LAMBDA parameter, with its prefix:
    /// `TaxRate`, `Budget!Totals`, `[1]!Rate`, `_xlpm.x`.
    Name,
    /// The name of a called function, the `(` that follows it excluded:
    /// `SUM`, `_xlfn.XLOOKUP`, `_xlws.SORT`.
    Function,
    /// A table reference with its table name, if any:
    /// `Expenses[[#This Row],[Quantity]]`, `[Amount]`.
    StructuredReference,
    /// `+ - * / ^ & = <> < <= > >= %`, the range `:`, and the displayed
    /// form's `@` (implicit intersection) and `#` (spilled range).
    Operator,
    Comma,
    Semicolon,
    OpenParen,
    CloseParen,
    OpenBrace,
    CloseBrace,
}

/// A token and where it is in the formula, in bytes.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Range<usize>,
}

/// The error values a formula can hold (MS-XLSX 2.2.2 `error-constant`, and
/// the ones later versions of Excel show).
pub(crate) const ERROR_VALUES: [&str; 16] = [
    "#NULL!",
    "#DIV/0!",
    "#VALUE!",
    "#REF!",
    "#NAME?",
    "#NUM!",
    "#N/A",
    "#GETTING_DATA",
    "#SPILL!",
    "#CALC!",
    "#FIELD!",
    "#BLOCKED!",
    "#UNKNOWN!",
    "#CONNECT!",
    "#BUSY!",
    "#PYTHON!",
];

/// The tokens of `formula`, stored or displayed, without its leading `=`.
/// Fails on what cannot be a token: an unterminated text or sheet name, an
/// unclosed table reference, a character no formula holds.
pub fn tokenize(formula: &str) -> Result<Vec<Token>, SyntaxError> {
    let mut lexer = Lexer {
        text: formula,
        at: 0,
        tokens: Vec::new(),
    };
    while lexer.at < formula.len() {
        lexer.next_token()?;
    }
    Ok(lexer.tokens)
}

struct Lexer<'a> {
    text: &'a str,
    at: usize,
    tokens: Vec<Token>,
}

impl Lexer<'_> {
    fn rest(&self) -> &str {
        &self.text[self.at..]
    }

    fn char_at(&self, at: usize) -> Option<char> {
        self.text.get(at..)?.chars().next()
    }

    fn push(&mut self, kind: TokenKind, end: usize) {
        self.tokens.push(Token {
            kind,
            span: self.at..end,
        });
        self.at = end;
    }

    fn unexpected(&self) -> SyntaxError {
        let c = self.char_at(self.at).unwrap_or(' ');
        SyntaxError::new(self.text, self.at, format!("`{c}` cannot be read here"))
    }

    /// Whether the previous token ends right here and can take a spill `#`.
    fn follows_spillable(&self) -> bool {
        self.tokens.last().is_some_and(|token| {
            token.span.end == self.at
                && matches!(
                    token.kind,
                    TokenKind::Reference | TokenKind::Name | TokenKind::CloseParen
                )
        })
    }

    fn next_token(&mut self) -> Result<(), SyntaxError> {
        let c = self.char_at(self.at).expect("a character is left");
        let at = self.at;
        match c {
            ' ' | '\t' | '\r' | '\n' => {
                let length = self
                    .rest()
                    .bytes()
                    .take_while(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
                    .count();
                self.push(TokenKind::Whitespace, at + length);
            }
            '"' => {
                let end = self.quoted(at, '"').ok_or_else(|| {
                    SyntaxError::new(
                        self.text,
                        at,
                        "a text in quotes is not closed: end it with `\"`",
                    )
                })?;
                self.push(TokenKind::Text, end);
            }
            '#' if self.follows_spillable() => self.push(TokenKind::Operator, at + 1),
            '#' => {
                let end = self.error_value(at).ok_or_else(|| self.unexpected())?;
                self.push(TokenKind::Error, end);
            }
            '(' => self.push(TokenKind::OpenParen, at + 1),
            ')' => self.push(TokenKind::CloseParen, at + 1),
            '{' => self.push(TokenKind::OpenBrace, at + 1),
            '}' => self.push(TokenKind::CloseBrace, at + 1),
            ',' => self.push(TokenKind::Comma, at + 1),
            ';' => self.push(TokenKind::Semicolon, at + 1),
            '<' | '>' => {
                let two = self.rest().starts_with("<>")
                    || self.rest().starts_with("<=")
                    || self.rest().starts_with(">=");
                self.push(TokenKind::Operator, at + 1 + usize::from(two));
            }
            '+' | '-' | '*' | '/' | '^' | '&' | '=' | '%' | '@' | ':' => {
                self.push(TokenKind::Operator, at + 1)
            }
            '\'' => {
                let end = self
                    .quoted(at, '\'')
                    .filter(|&end| self.text[end..].starts_with('!'))
                    .ok_or_else(|| {
                        SyntaxError::new(
                            self.text,
                            at,
                            "a sheet name in quotes must be closed and followed by `!`",
                        )
                    })?;
                self.after_prefix(end + 1)?;
            }
            '[' => self.bracket()?,
            '!' => self.after_prefix(at + 1)?,
            '$' => {
                let (end, _) = target_at(self.text, at).ok_or_else(|| self.unexpected())?;
                self.push(TokenKind::Reference, end);
            }
            '0'..='9' | '.' => self.number_or_rows()?,
            c if is_name_start(c) => self.word()?,
            _ => return Err(self.unexpected()),
        }
        Ok(())
    }

    /// The end of the text quoted by `quote` at `at`, a doubled quote kept
    /// inside.
    fn quoted(&self, at: usize, quote: char) -> Option<usize> {
        let mut index = at + 1;
        loop {
            let found = self.text[index..].find(quote)?;
            index += found + 1;
            if self.text[index..].starts_with(quote) {
                index += 1;
            } else {
                return Some(index);
            }
        }
    }

    /// The end of the error value at `at`: a known one in any case, or
    /// another `#WORD!` or `#WORD?` the parser names.
    fn error_value(&self, at: usize) -> Option<usize> {
        let rest = &self.text[at..];
        for value in ERROR_VALUES {
            if rest.len() >= value.len() && rest[..value.len()].eq_ignore_ascii_case(value) {
                return Some(at + value.len());
            }
        }
        let word = rest[1..]
            .bytes()
            .take_while(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'/'))
            .count();
        (word > 0 && matches!(rest.as_bytes().get(1 + word), Some(b'!' | b'?')))
            .then_some(at + word + 2)
    }

    /// A run of name characters at `at`, and where it ends.
    fn word_end(&self, at: usize) -> usize {
        self.text[at..]
            .char_indices()
            .find(|&(_, c)| !is_name_char(c))
            .map_or(self.text.len(), |(index, _)| at + index)
    }

    /// The end of an unquoted sheet prefix at `at`, `!` included: `Sheet1!`,
    /// or `Sheet1:Sheet3!` when its first sheet cannot be read as a cell.
    fn sheet_prefix_end(&self, at: usize) -> Option<usize> {
        let end = self.word_end(at);
        if end == at {
            return None;
        }
        if self.text[end..].starts_with('!') {
            return Some(end + 1);
        }
        if self.text[end..].starts_with(':') && target_at(self.text, at).is_none() {
            let last = self.word_end(end + 1);
            if last > end + 1 && self.text[last..].starts_with('!') {
                return Some(last + 1);
            }
        }
        None
    }

    /// What follows a sheet or workbook prefix ending at `at`: a reference,
    /// `#REF!`, a name, a function or a table reference.
    fn after_prefix(&mut self, at: usize) -> Result<(), SyntaxError> {
        let rest = &self.text[at..];
        if rest.len() >= 5 && rest[..5].eq_ignore_ascii_case("#REF!") {
            self.push(TokenKind::Reference, at + 5);
            return Ok(());
        }
        if let Some((end, _)) = target_at(self.text, at) {
            self.push(TokenKind::Reference, end);
            return Ok(());
        }
        if self.char_at(at).is_some_and(is_name_start) {
            let end = self.word_end(at);
            return self.named(end);
        }
        Err(SyntaxError::new(
            self.text,
            at,
            "a sheet or workbook prefix must be followed by a cell or a name",
        ))
    }

    /// A name ending at `end`: a function before `(`, a table before `[`,
    /// else a name.
    fn named(&mut self, end: usize) -> Result<(), SyntaxError> {
        match self.char_at(end) {
            Some('(') => self.push(TokenKind::Function, end),
            Some('[') => {
                let close = self.brackets(end)?;
                self.push(TokenKind::StructuredReference, close);
            }
            _ => self.push(TokenKind::Name, end),
        }
        Ok(())
    }

    /// The end of the bracketed part of a table reference at `at`: nested
    /// brackets counted, `'` escaping the character after it.
    fn brackets(&self, at: usize) -> Result<usize, SyntaxError> {
        let mut depth = 0usize;
        let mut chars = self.text[at..].char_indices();
        while let Some((index, c)) = chars.next() {
            match c {
                '\'' => {
                    chars.next();
                }
                '[' => depth += 1,
                ']' => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(at + index + 1);
                    }
                }
                _ => {}
            }
        }
        Err(SyntaxError::new(
            self.text,
            at,
            "a table reference is not closed: end it with `]`",
        ))
    }

    /// `[` at an operand: a workbook prefix (`[1]Sheet1!A1`, `[1]!Name`)
    /// or a table reference without its table name (`[Amount]`).
    fn bracket(&mut self) -> Result<(), SyntaxError> {
        let at = self.at;
        if let Some(close) = self.text[at..].find(']').map(|index| at + index + 1)
            && !self.text[at + 1..close - 1].contains('[')
        {
            if self.text[close..].starts_with('!') {
                return self.after_prefix(close + 1);
            }
            if let Some(end) = self.sheet_prefix_end(close) {
                return self.after_prefix(end);
            }
        }
        let close = self.brackets(at)?;
        self.push(TokenKind::StructuredReference, close);
        Ok(())
    }

    /// At a digit or `.`: whole rows (`2:5`), a sheet prefix that starts with
    /// a digit (`2024!A1`, as some producers write it), or a number.
    fn number_or_rows(&mut self) -> Result<(), SyntaxError> {
        let at = self.at;
        if let Some((end, _)) = target_at(self.text, at) {
            self.push(TokenKind::Reference, end);
            return Ok(());
        }
        if let Some(end) = self.sheet_prefix_end(at) {
            return self.after_prefix(end);
        }
        let bytes = self.text.as_bytes();
        let digits = |from: usize| {
            bytes[from..]
                .iter()
                .take_while(|b| b.is_ascii_digit())
                .count()
        };
        let mut end = at + digits(at);
        if bytes.get(end) == Some(&b'.') {
            end += 1 + digits(end + 1);
        }
        if end == at + 1 && bytes[at] == b'.' {
            return Err(self.unexpected());
        }
        if matches!(bytes.get(end), Some(b'e' | b'E')) {
            let sign = usize::from(matches!(bytes.get(end + 1), Some(b'+' | b'-')));
            let exponent = digits(end + 1 + sign);
            if exponent > 0 {
                end += 1 + sign + exponent;
            }
        }
        self.push(TokenKind::Number, end);
        Ok(())
    }

    /// At a name character: a sheet prefix, a reference, a boolean, a
    /// function, a table reference or a name.
    fn word(&mut self) -> Result<(), SyntaxError> {
        let at = self.at;
        if let Some(end) = self.sheet_prefix_end(at) {
            return self.after_prefix(end);
        }
        if let Some((end, _)) = target_at(self.text, at) {
            self.push(TokenKind::Reference, end);
            return Ok(());
        }
        let end = self.word_end(at);
        let word = &self.text[at..end];
        if (word.eq_ignore_ascii_case("TRUE") || word.eq_ignore_ascii_case("FALSE"))
            && self.char_at(end) != Some('(')
        {
            self.push(TokenKind::Boolean, end);
            return Ok(());
        }
        self.named(end)
    }
}
