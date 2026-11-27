//! Excel-compatible workbooks: open and save without loss.
//!
//! A workbook opens from a path or bytes and is told by its content, never
//! its extension: a legacy `.xls`, a binary `.xlsb`, an OpenDocument
//! spreadsheet, a Word or PowerPoint package and a password-protected file
//! are each refused with a plain message naming the way out. Strict Open
//! XML reads as Transitional and keeps its namespaces.
//!
//! What the facade does not model is kept as it was read: a save without an
//! edit writes every part back byte for byte. A save is staged in a sibling
//! file and renamed over its path, and refuses the file the workbook was
//! opened from unless [`SaveOptions::overwrite_source`] is set.
//!
//! Positions are 0-based, like every index of the oxoffice packages; the
//! command line counts from 1.

mod edit;
mod grid;
mod new_workbook;
mod open;
mod page_setup;
mod save;
mod sheet;
mod signature;

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use oxoffice_opc::{OpcError, OpcPackage};
use oxoffice_xlsx_oxml::workbook::WorkbookXml;
use thiserror::Error;

pub use edit::WorksheetMut;
pub use page_setup::{
    CellComments, HeaderFooter, Orientation, PageBreak, PageMargins, PageOrder, PageSetup,
    PrintErrors,
};
pub use save::{SaveOptions, WorkbookClass};
pub use sheet::{SheetKind, SheetState, Worksheet};

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
