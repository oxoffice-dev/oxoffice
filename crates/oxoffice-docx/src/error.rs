//! Error types for the rdocx high-level API.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("OPC package error: {0}")]
    Opc(#[from] oxoffice_opc::OpcError),

    #[error("OXML parsing error: {0}")]
    Oxml(#[from] oxoffice_docx_oxml::OxmlError),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("layout error: {0}")]
    Layout(#[from] oxoffice_layout::LayoutError),

    #[error("PDF conformance error: {0}")]
    Pdf(#[from] oxoffice_pdf::PdfError),

    #[error("raster export error: {0}")]
    Raster(#[from] oxoffice_pdf::RasterError),

    #[error("document has no main document part")]
    NoDocumentPart,

    #[error("image dimensions are unavailable for {filename}")]
    UnavailableImageDimensions { filename: String },

    #[error("RTF parse error at byte {offset}: {message}")]
    Rtf { offset: usize, message: String },

    #[error("HTML import error at {location}: {message}")]
    Html { location: String, message: String },

    #[error("MHTML error in {part:?} at byte {offset}: {message}")]
    Mhtml {
        part: Option<String>,
        offset: u64,
        message: String,
    },

    #[error("ODT import error in {part:?} at byte {offset}: {message}")]
    Odt {
        part: Option<String>,
        offset: u64,
        message: String,
    },

    #[error("{operation} failed: {message}")]
    InvalidEmbeddedMutation {
        operation: &'static str,
        message: String,
    },

    #[error("story location error: {0}")]
    Story(#[from] crate::document::StoryError),

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;
