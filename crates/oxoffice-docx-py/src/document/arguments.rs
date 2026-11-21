//! Python arguments checked and converted: documents and streams, lengths, indexes, dates and render options.

use super::*;

pub(super) fn binary_stream_bytes(value: &Bound<'_, PyAny>, seek: bool) -> PyResult<Vec<u8>> {
    if seek {
        value.call_method1("seek", (0,)).map_err(|error| {
            if error.is_instance_of::<pyo3::exceptions::PyAttributeError>(value.py()) {
                PyTypeError::new_err(
                    "Document requires a seekable binary stream; use Document.from_bytes(stream.read())",
                )
            } else {
                error
            }
        })?;
    }
    let bytes = value.call_method0("read").map_err(|error| {
        if error.is_instance_of::<pyo3::exceptions::PyAttributeError>(value.py()) {
            PyTypeError::new_err(
                "use a binary stream, filesystem path or Document.from_bytes(bytes)",
            )
        } else {
            error
        }
    })?;
    bytes
        .cast::<PyBytes>()
        .map(|bytes| bytes.as_bytes().to_vec())
        .map_err(|_| {
            PyTypeError::new_err(
                "stream.read() must return binary bytes; use Document.from_bytes(bytes)",
            )
        })
}

pub(super) fn open_document_input(
    py: Python<'_>,
    value: &Bound<'_, PyAny>,
) -> PyResult<oxoffice_docx::Document> {
    if let Ok(path) = value.extract::<PathBuf>() {
        return oxoffice_docx::Document::open(path).map_err(|error| docx_to_pyerr(py, error));
    }
    oxoffice_docx::Document::from_bytes(&binary_stream_bytes(value, true)?)
        .map_err(|error| docx_to_pyerr(py, error))
}

pub(super) fn checked_heading_level(value: &Bound<'_, PyAny>) -> PyResult<i64> {
    if value.is_instance_of::<PyBool>() {
        return Err(PyTypeError::new_err(
            "heading level must be an integer from 0 through 9, not bool",
        ));
    }
    value.extract()
}

impl PyDocument {
    pub(super) fn direct_content_index(
        slf: &Py<Self>,
        py: Python<'_>,
        content: &Bound<'_, PyAny>,
        argument: &str,
    ) -> PyResult<usize> {
        if let Ok(paragraph) = content.cast::<PyParagraph>() {
            let paragraph = paragraph.borrow();
            if !paragraph.belongs_to(py, slf) {
                return Err(PyValueError::new_err(
                    "content handle belongs to a different document",
                ));
            }
            let paragraph_index = match paragraph.validate(py)? {
                crate::paragraph::ParagraphLocation::Body(index) => index,
                crate::paragraph::ParagraphLocation::Cell { .. }
                | crate::paragraph::ParagraphLocation::Related(_) => {
                    return Err(PyValueError::new_err(
                        "content handle is not a direct body child",
                    ));
                }
            };
            return slf
                .borrow(py)
                .inner
                .content_index_of_paragraph(paragraph_index)
                .ok_or_else(|| PyValueError::new_err("content handle is not a direct body child"));
        }
        if let Ok(table) = content.cast::<PyTable>() {
            let table = table.borrow();
            if !table.belongs_to(py, slf) {
                return Err(PyValueError::new_err(
                    "content handle belongs to a different document",
                ));
            }
            return table.direct_body_index(py);
        }
        Err(PyTypeError::new_err(format!(
            "{argument} must be a Paragraph or Table handle"
        )))
    }
}

impl PyDocument {
    /// Check the names and the section of one header or footer variant.
    pub(super) fn section_story_target(
        &self,
        section_index: usize,
        kind: &str,
        variant: &str,
    ) -> PyResult<(oxoffice_docx::HeaderFooterKind, oxoffice_docx::HdrFtrType)> {
        let kind = header_footer_kind_from_name(kind)?;
        let variant = header_footer_variant_from_name(variant)?;
        if section_index >= self.inner.section_count() {
            return Err(PyIndexError::new_err("section index out of range"));
        }
        Ok((kind, variant))
    }
}

/// The date `date` and `undated` give a new comment or reply: the current
/// time when neither is given. Both together are refused.
pub(super) fn comment_date(
    date: Option<&str>,
    undated: bool,
) -> PyResult<oxoffice_docx::CommentDate<'_>> {
    match (date, undated) {
        (Some(_), true) => Err(PyValueError::new_err(
            "date and undated=True contradict each other: pass one of them",
        )),
        (Some(date), false) => Ok(oxoffice_docx::CommentDate::At(date)),
        (None, true) => Ok(oxoffice_docx::CommentDate::Undated),
        (None, false) => Ok(oxoffice_docx::CommentDate::Now),
    }
}

/// Extract a direct body index, naming the accepted forms on a type error.
pub(super) fn body_index_argument(
    value: &Bound<'_, PyAny>,
    message: &'static str,
) -> PyResult<usize> {
    value.extract::<usize>().map_err(|error| {
        if error.is_instance_of::<PyTypeError>(value.py()) {
            PyTypeError::new_err(message)
        } else {
            error
        }
    })
}

pub(crate) fn checked_section_length(
    value: Option<&Bound<'_, PyAny>>,
    name: &str,
) -> PyResult<Option<i64>> {
    value
        .map(|value| {
            if value.is_instance_of::<PyBool>() {
                return Err(PyTypeError::new_err(format!(
                    "{name} must be an integer EMU length, not bool"
                )));
            }
            value.extract::<i64>()
        })
        .transpose()
}

// Public update_section has an explicit-unit contract. Other consumers of
// checked_section_length, including live Section setters, keep their EMU API.
pub(super) fn checked_update_section_length(
    py: Python<'_>,
    value: Option<&Bound<'_, PyAny>>,
    name: &str,
) -> PyResult<Option<i64>> {
    if let Some(value) = value
        && !value.is_instance_of::<pyo3::types::PyInt>()
    {
        return Err(PyTypeError::new_err(format!(
            "{name} requires Length or zero; use Twips or Emu"
        )));
    }
    let emu = checked_section_length(value, name)?;
    if let Some(value) = value
        && emu != Some(0)
        && !value.is_instance(&py.import("oxoffice.docx.shared")?.getattr("Length")?)?
    {
        return Err(PyValueError::new_err(format!(
            "update_section {name} nonzero geometry requires explicit Length; use Twips(1440) or Emu(value)"
        )));
    }
    Ok(emu)
}

/// A section length the caller gave, its explicit value, or its layout default.
pub(super) fn given_or_current_length(
    given: Option<i64>,
    current: Option<oxoffice_docx::Twips>,
    default: oxoffice_docx::Twips,
) -> oxoffice_docx::Length {
    match (given, current) {
        (Some(emu), _) => oxoffice_docx::Length::emu(emu),
        (None, Some(twips)) => oxoffice_docx::Length::twips(twips.0),
        (None, None) => oxoffice_docx::Length::twips(default.0),
    }
}

/// Fillable forms need the accepted view: a field value would mix inserted
/// and deleted text. Refused here, before Rust, like the other clashes.
pub(super) fn check_fillable_forms(fillable_forms: bool, revision_view: &str) -> PyResult<bool> {
    if fillable_forms && revision_view == "tracked" {
        return Err(PyValueError::new_err(
            "fillable_forms applies only with revision_view=\"accepted\"",
        ));
    }
    Ok(fillable_forms)
}

pub(super) fn parse_render_options(
    revision_view: &str,
    revision_style: &str,
) -> PyResult<oxoffice_docx::RenderOptions> {
    let revision_view = match revision_view {
        "accepted" => oxoffice_docx::RevisionView::Accepted,
        "tracked" => oxoffice_docx::RevisionView::Tracked,
        other => {
            return Err(PyValueError::new_err(format!(
                "unknown revision view {other:?}, expected accepted or tracked"
            )));
        }
    };
    let revision_style = match revision_style {
        "plain" => oxoffice_docx::RevisionStyle::Plain,
        "legal" => oxoffice_docx::RevisionStyle::Legal,
        other => {
            return Err(PyValueError::new_err(format!(
                "unknown revision style {other:?}, expected plain or legal"
            )));
        }
    };
    if revision_style != oxoffice_docx::RevisionStyle::Plain
        && revision_view != oxoffice_docx::RevisionView::Tracked
    {
        return Err(PyValueError::new_err(
            "revision_style applies only with revision_view=\"tracked\"",
        ));
    }
    Ok(oxoffice_docx::RenderOptions::default()
        .with_revision_view(revision_view)
        .with_revision_style(revision_style))
}

pub(super) fn parse_raster_format(
    format: &str,
    quality: u8,
    transparent: bool,
) -> oxoffice_docx::Result<oxoffice_docx::RasterFormat> {
    match format {
        "png" => Ok(oxoffice_docx::RasterFormat::Png {
            transparent_background: transparent,
        }),
        "jpg" | "jpeg" => Ok(oxoffice_docx::RasterFormat::Jpeg { quality }),
        "tif" | "tiff" => Ok(oxoffice_docx::RasterFormat::Tiff),
        other => Err(oxoffice_docx::Error::Other(format!(
            "unknown raster format {other:?}, expected png, jpeg, or tiff"
        ))),
    }
}

pub(super) fn parse_comparison_granularity(
    name: &str,
) -> oxoffice_docx::Result<oxoffice_docx::ComparisonGranularity> {
    match name {
        "run" => Ok(oxoffice_docx::ComparisonGranularity::Run),
        "word" => Ok(oxoffice_docx::ComparisonGranularity::Word),
        "character" => Ok(oxoffice_docx::ComparisonGranularity::Character),
        other => Err(oxoffice_docx::Error::Other(format!(
            "unknown comparison granularity {other:?}, expected run, word, or character"
        ))),
    }
}

/// Story names follow `Story.kind`, so `body` selects the main story.
pub(super) fn parse_comparison_story(
    name: &str,
) -> oxoffice_docx::Result<oxoffice_docx::ComparisonStoryKind> {
    match name {
        "body" => Ok(oxoffice_docx::ComparisonStoryKind::Main),
        "header" => Ok(oxoffice_docx::ComparisonStoryKind::Header),
        "footer" => Ok(oxoffice_docx::ComparisonStoryKind::Footer),
        "comment" => Ok(oxoffice_docx::ComparisonStoryKind::Comment),
        "text_box" => Ok(oxoffice_docx::ComparisonStoryKind::TextBox),
        "footnote" => Ok(oxoffice_docx::ComparisonStoryKind::Footnote),
        "endnote" => Ok(oxoffice_docx::ComparisonStoryKind::Endnote),
        other => Err(oxoffice_docx::Error::Other(format!(
            "unknown comparison story {other:?}, expected body, header, footer, comment, \
             text_box, footnote, or endnote"
        ))),
    }
}
