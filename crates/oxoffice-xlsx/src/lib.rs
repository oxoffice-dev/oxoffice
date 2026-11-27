//! Excel-compatible workbooks: open, read and save without loss.
//!
//! A workbook opens from a path or bytes and is told by its content, never
//! its extension: a legacy `.xls`, a binary `.xlsb`, an OpenDocument
//! spreadsheet, a Word or PowerPoint package and a password-protected file
//! are each refused with a plain message naming the way out. Strict Open
//! XML reads as Transitional and keeps its namespaces.
//!
//! What the facade does not model is kept as it was read: a save without an
//! edit writes every part back byte for byte (a package signature that
//! already fails verification gets its mark, as in docx and pptx). A save is
//! staged in a sibling file and renamed over its path, and refuses the file
//! the workbook was opened from unless [`SaveOptions::overwrite_source`] is
//! set.
//!
//! A cell reads as a snapshot ([`Cell`]): its value (for a formula, the
//! value its producer cached), its formula in the form Excel displays and
//! the form the file stores, the text Excel displays for it and its
//! resolved style. Sheets also give their used range, merges, comments and
//! page setup, and the workbook its text as Markdown or a sheet as CSV.
//!
//! ```no_run
//! use oxoffice_xlsx::Workbook;
//!
//! let workbook = Workbook::open("review.xlsx")?;
//! let sheet = workbook.sheet("Orders").expect("a sheet named Orders");
//! let total = sheet.cell("F38")?;
//! println!("{} = {}", total.formula.unwrap_or_default(), total.text);
//! for row in sheet.values("A1:G5")? {
//!     println!("{row:?}");
//! }
//! # Ok::<(), oxoffice_xlsx::Error>(())
//! ```
//!
//! Positions are 0-based, like every index of the oxoffice packages; A1
//! references are the spreadsheet's own, from 1, and the command line counts
//! from 1.

mod edit;
mod grid;
mod new_workbook;
mod cell;
mod comments;
mod open;
mod page_setup;
mod read;
mod save;
mod sheet;
mod signature;
mod style;
mod text;

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use oxoffice_opc::{OpcError, OpcPackage};
use oxoffice_xlsx_oxml::workbook::WorkbookXml;
use thiserror::Error;

pub use edit::WorksheetMut;
pub use cell::{Cell, CellError, FormulaKind, Value};
pub use comments::{Comment, CommentKind, Reply};
pub use oxoffice_numfmt::DateTime;
pub use oxoffice_xlsx_oxml::{CellRange, CellRef};
pub use page_setup::{
    CellComments, HeaderFooter, Orientation, PageBreak, PageMargins, PageOrder, PageSetup,
    PrintErrors,
};
pub use read::MAX_BULK_CELLS;
pub use save::{SaveOptions, WorkbookClass};
pub use sheet::{SheetKind, SheetState, Worksheet};
pub use style::{
    Alignment, Border, BorderSide, Color, ColorSource, Fill, Font, GradientStop, Protection,
    ResolvedStyle,
};
pub use text::CsvOptions;

/// A result of the `oxoffice-xlsx` facade.
pub type Result<T> = std::result::Result<T, Error>;

/// An error opening, reading or saving a workbook.
#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Package(#[from] OpcError),

    #[error("{part} is not well-formed SpreadsheetML: {message}")]
    MalformedPart { part: String, message: String },

    #[error("{0}")]
    InvalidArgument(String),

    #[error("{0}")]
    Refused(String),

    #[error("the workbook is password-protected: open it with a password")]
    PasswordProtected,

    #[error("the workbook is not password-protected: open it without a password")]
    NotPasswordProtected,

    #[error(
        "a legacy Excel 97-2003 workbook (.xls): save it as .xlsx in Excel or LibreOffice first"
    )]
    LegacyWorkbook,

    #[error("a legacy Word 97-2003 document (.doc), not a workbook")]
    LegacyDocument,

    #[error("a legacy PowerPoint 97-2003 presentation (.ppt), not a workbook")]
    LegacyPresentation,

    #[error("a compound file that holds no workbook")]
    CompoundFile,

    #[error("a binary workbook (.xlsb): save it as .xlsx first")]
    BinaryWorkbook,

    #[error("an OpenDocument spreadsheet (.ods): save it as .xlsx first")]
    OpenDocumentSpreadsheet,

    #[error("a Word document, not a workbook: open it with oxoffice docx")]
    WordDocument,

    #[error("a PowerPoint presentation, not a workbook: open it with oxoffice pptx")]
    PowerPointPresentation,

    #[error("not a workbook: {}", match content_type {
        Some(content_type) => format!("the main part is {content_type}"),
        None => "the package names no main part".to_owned(),
    })]
    NotAWorkbook { content_type: Option<String> },

    #[error("{source_part}: relationship {relationship_id} is missing")]
    MissingRelationship {
        source_part: String,
        relationship_id: String,
    },

    #[error("package part is missing: {part}")]
    MissingPart { part: String },

    #[error(
        "refusing to save over {}, the file this workbook was opened from; pass overwrite_source to replace it",
        path.display()
    )]
    OverwritesSource { path: PathBuf },

    #[error(
        "{} cannot be written: the workbook carries a VBA project and the extension names a macro-free workbook, save it as .xlsm or .xltm",
        path.display()
    )]
    MacroFreeExtension { path: PathBuf },

    #[error("`{reference}` is not a cell or a range: write B3, A1:C10, A:C or 2:5")]
    InvalidReference { reference: String },

    #[error("`{reference}` is not a cell: write B3 or $B$3")]
    NotACell { reference: String },

    #[error("sheet {sheet} is a {}: it holds no cells to read", kind.as_str())]
    NotAWorksheet { sheet: String, kind: SheetKind },

    #[error(
        "{range} holds more than {} cells: read a smaller range",
        read::MAX_BULK_CELLS
    )]
    RangeTooLarge { range: String },
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Error::Package(OpcError::Io(error))
    }
}

/// How dates are counted: from 1900 (Excel's default) or from 1904
/// (`workbookPr date1904`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DateSystem {
    Date1900,
    Date1904,
}

/// An Excel-compatible workbook. See the [crate](crate) documentation.
#[derive(Clone, Debug)]
pub struct Workbook {
    package: OpcPackage,
    /// The file the workbook was opened from, which a save refuses.
    source_path: Option<PathBuf>,
    /// The main part, `/xl/workbook.xml` in every producer's files.
    workbook_part: String,
    class: WorkbookClass,
    workbook: WorkbookXml,
    sheets: Vec<sheet::SheetEntry>,
    /// Whether an edit has changed the workbook since it was opened or
    /// made: a save writes what edits staged.
    edited: bool,
    /// Whether the workbook asks Excel to calculate it on open and has lost
    /// its calculation chain, which an edit of a workbook that holds a
    /// formula does.
    calculates: bool,
    /// Whether any sheet holds a formula, found by one scan of the sheets as
    /// read, at the first edit.
    formulas: OnceLock<bool>,
    /// Whether the package signature was known invalid when the workbook was
    /// opened.
    signature_invalid: bool,
    /// The package as opened, when it carries a package signature: a save
    /// that differs from it invalidates the signature.
    signature_baseline: Option<Arc<OpcPackage>>,
    /// The parts cells read, each read the first time a cell needs it.
    read: read::ReadParts,
}

impl Workbook {
    /// How dates are counted in this workbook.
    pub fn date_system(&self) -> DateSystem {
        if self.workbook.date1904() {
            DateSystem::Date1904
        } else {
            DateSystem::Date1900
        }
    }

    /// The workbook, template or macro-enabled class of the package, from
    /// its main content type.
    pub fn class(&self) -> WorkbookClass {
        self.class
    }
}

/// A value read once, on first use, kept with the error its read gave.
type Lazy<T> = OnceLock<std::result::Result<T, String>>;
