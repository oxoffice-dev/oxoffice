//! PDF renderer for format-neutral OOXML layout output.
//!
//! Converts `LayoutResult` (positioned page frames with glyph runs, lines,
//! rectangles, and images) into a PDF document.

mod conformance;
mod font;
mod forms;
mod image;
pub mod raster;
mod structure;
mod writer;

use oxoffice_layout::LayoutResult;

pub use forms::untypable_form_text;
pub use image::{ImageDrawClass, classify_image_draw};

pub use conformance::{PdfConformance, PdfError, PdfOptions};
pub use raster::{RasterError, RasterFormat, RasterOptions, RasterOutput};

/// Render a laid-out document to PDF bytes.
///
/// The `LayoutResult` must contain all pages, fonts, metadata, and outlines
/// produced by a format-specific layout engine. If a contributing font cannot
/// be prepared, this legacy operation returns an empty vector without a partial
/// PDF. Use [`render_to_pdf_checked`] to receive the actual preparation error.
pub fn render_to_pdf(layout: &LayoutResult) -> Vec<u8> {
    writer::write_pdf(layout)
}

/// Render PDF bytes after preparing every contributing font exactly once.
///
/// A missing or unembeddable selected instance returns an error before output.
/// This normal PDF operation does not impose archival action or paint rules.
pub fn render_to_pdf_checked(layout: &LayoutResult) -> Result<Vec<u8>, PdfError> {
    let prepared_fonts = font::prepare_fonts(layout)?;
    Ok(writer::write_pdf_impl(layout, None, prepared_fonts))
}

/// Render a laid-out document to one explicitly selected archival PDF profile.
///
/// The complete layout is validated before the writer allocates output bytes.
pub fn render_to_pdf_with_options(
    layout: &LayoutResult,
    options: PdfOptions,
) -> Result<Vec<u8>, PdfError> {
    let prepared_fonts = conformance::preflight(layout)?;
    Ok(writer::write_pdf_impl(
        layout,
        Some(options.profile),
        prepared_fonts,
    ))
}

/// Render a single page to PNG bytes.
///
/// # Arguments
/// * `layout` - The format-neutral layout result to rasterise
/// * `page_index` - 0-based page index
/// * `dpi` - Resolution (72 = 1:1, 150 = standard, 300 = high quality)
pub fn render_page_to_png(layout: &LayoutResult, page_index: usize, dpi: f64) -> Option<Vec<u8>> {
    raster::render_page_to_png(layout, page_index, dpi)
}

/// Render all pages to PNG bytes.
pub fn render_all_pages(layout: &LayoutResult, dpi: f64) -> Vec<Vec<u8>> {
    raster::render_all_pages(layout, dpi)
}

/// Render selected zero-based pages to the requested raster image format.
///
/// Page selections preserve caller order. Empty selections, duplicate pages,
/// out-of-range pages, invalid DPI, and invalid format options are rejected
/// before any output is encoded.
pub fn render_pages(
    layout: &LayoutResult,
    page_indices: &[usize],
    options: RasterOptions,
) -> Result<RasterOutput, RasterError> {
    raster::render_pages(layout, page_indices, options)
}
