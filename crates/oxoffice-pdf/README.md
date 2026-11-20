# oxoffice-pdf

Render format-neutral `LayoutResult` pages to PDF, archival PDF/A, or raster
images.

## Capabilities

- PDF output from positioned page elements.
- PDF/A-2b and PDF/A-3b preflight and rendering.
- Font embedding and subsetting, metadata, outlines, links, and structure.
- Selected-page PNG and JPEG plus multi-page TIFF output.
- Tagged reading order, PDF/A output intents, gradients, clipping, opacity,
  transformed groups, and shared raster composition use the same page model.

## Use it when

Use this crate when a custom OOXML frontend already produces shared layout frames. Use `oxoffice_docx::Document::to_pdf` or `oxoffice_pptx::Presentation::to_pdf_deterministic` for normal document conversion.

## Relationship

It is an
output backend and does not open OOXML packages or lay out DOCX and PPTX
content. Raster page indices are zero-based.

## Example

```rust,no_run
use oxoffice_layout::LayoutResult;
use oxoffice_pdf::render_to_pdf;

let layout = LayoutResult::new(Vec::new(), Vec::new(), None, Vec::new());
let pdf = render_to_pdf(&layout);
assert!(pdf.starts_with(b"%PDF-"));
```

Add `oxoffice-pdf = "0.1.0"` and `oxoffice-layout = "0.1.0"` to your dependencies. See the [renderer API](https://docs.rs/oxoffice-pdf) for the accepted layout model.
