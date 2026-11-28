//! SpreadsheetML parts read and written without loss: the workbook part,
//! shared strings, styles and worksheets of an `.xlsx` package, and the
//! comment parts, read only.
//!
//! What a part's model reads goes through typed accessors over its elements
//! as written; what it does not model is kept as written. A part is written
//! from its model only when an edit changed it, as its source bytes with the
//! changes spliced in. Strict Open XML reads as Transitional
//! ([`namespaces`]) and keeps its namespaces.
#![allow(non_camel_case_types)]

pub mod app_properties;
pub mod comments;
pub mod namespaces;
mod part;
pub mod reference;
pub mod shared_strings;
pub mod styles;
pub mod workbook;
pub mod worksheet;
mod xml;
pub mod xstring;

pub use oxoffice_core::OxmlError;
pub use part::{Located, RecordList, TypedPart};
pub use reference::{CellRange, CellRef};
pub use xml::{
    Element, Lexical, Node, Tag, format_f64, parse_bool, parse_f64, parse_u32, prefix_of_name,
    qualified,
};
