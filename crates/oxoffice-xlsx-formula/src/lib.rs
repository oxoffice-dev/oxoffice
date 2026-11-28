//! Spreadsheet formulas as a workbook stores them and as Excel displays them.
//!
//! A formula is read as stored in a SpreadsheetML part, without its `=`: the
//! text of a cell's `f`, a defined name, a validation or conditional format
//! formula, a chart reference. This crate gives
//!
//! - its tokens ([`tokenize`]), which joined give the formula back;
//! - its displayed form ([`to_displayed`]) and back ([`to_stored`]): future
//!   functions lose and take `_xlfn.` (`_xlfn._xlws.` for `FILTER`, `SORT` and
//!   `PY`), LET and LAMBDA parameters `_xlpm.` (`_xlop.y` is the optional
//!   `[y]`), a function passed as a value `_xleta.` (`GROUPBY(...,SUM)`),
//!   `_xlfn.SINGLE(x)` is `@x` and `_xlfn.ANCHORARRAY(x)` is `x#`;
//! - the formula of a shared formula's member, moved from its master
//!   ([`move_references`]);
//! - the syntax check a written formula passes ([`to_stored`],
//!   [`check_stored`]): operands, operators, calls and array constants, `,`
//!   between arguments, each built-in function's argument count
//!   ([`function`]), at most 255 arguments, 64 nested functions and 8,192
//!   characters;
//! - how large its result is, exactly or at most, when its text and the
//!   ranges it reads tell it ([`result_extent`]): what a dynamic array
//!   formula spills over.
//!
//! ```
//! use oxoffice_xlsx_formula::{move_references, to_displayed, to_stored};
//!
//! let stored = "_xlfn.XLOOKUP(A2,Rates!$A$2:$A$9,Rates!$B$2:$B$9)";
//! assert_eq!(to_displayed(stored), "XLOOKUP(A2,Rates!$A$2:$A$9,Rates!$B$2:$B$9)");
//! assert_eq!(to_stored("XLOOKUP(A2,Rates!$A$2:$A$9,Rates!$B$2:$B$9)")?, stored);
//! assert_eq!(move_references("D2*E2", 3, 0)?, "D5*E5");
//! assert_eq!(
//!     to_stored("IF(B2>0;1;0)").unwrap_err().to_string(),
//!     "`;` separates the rows of an array constant only: write `,` between arguments \
//!      (at character 8)"
//! );
//! # Ok::<(), oxoffice_xlsx_formula::SyntaxError>(())
//! ```
//!
//! The parser, the evaluator and the reference updates of inserted rows
//! come later, on these tokens. See `specs/2026-11-27-xlsx-open-read-edit.md`.

mod extent;
mod forms;
mod functions;
mod lexer;
mod mover;
mod parser;
mod reference;

pub use extent::{Extent, Shape, result_extent};
pub use forms::{check_stored, to_displayed, to_stored};
pub use functions::{Function, Prefix, function};
pub use lexer::{Token, TokenKind, tokenize};
pub use mover::move_references;

/// A formula that cannot be read, or that Excel would repair, with a plain
/// message and where in the formula it is.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{message} ({})", place(*.character))]
pub struct SyntaxError {
    message: String,
    offset: usize,
    /// The 1-based character the error is at, `None` at the end.
    character: Option<usize>,
}

impl SyntaxError {
    pub(crate) fn new(text: &str, offset: usize, message: impl Into<String>) -> Self {
        let character = (offset < text.len()).then(|| text[..offset].chars().count() + 1);
        SyntaxError {
            message: message.into(),
            offset,
            character,
        }
    }

    /// The same error in `text`, of which the formula read starts `shift`
    /// bytes in.
    pub(crate) fn shifted(self, text: &str, shift: usize) -> Self {
        SyntaxError::new(text, self.offset + shift, self.message)
    }

    /// What is wrong, without the position.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Where it is, in bytes from the start of the formula.
    pub fn offset(&self) -> usize {
        self.offset
    }
}

fn place(character: Option<usize>) -> String {
    match character {
        Some(character) => format!("at character {character}"),
        None => "at the end".to_owned(),
    }
}
