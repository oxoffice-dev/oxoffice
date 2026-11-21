# oxoffice-docx-layout

`oxoffice-docx-layout` turns semantic Word content into positioned pages. It resolves
styles and numbering, shapes text, lays out tables and notes, paginates
sections, and retains source provenance.

## Capabilities

- Word style, numbering, field, and paragraph resolution.
- Line breaking, tables, footnotes, endnotes, sections, and pagination.
- Deterministic bundled fonts plus system, embedded, and caller fonts.
- Page-reference lookup, Word source provenance, and top-level body extents.
- Shared `oxoffice-layout` output for downstream backends.
- Section-specific page geometry, columns, headers, footers, notes, floating
  tables, vertical text, and conditional table formatting retain Word meaning.

## Use it when

Use `layout_document_deterministic` for reproducible output with bundled fonts.
Applications that start from a DOCX file should normally call the rendering
methods on [`oxoffice_docx::Document`](https://docs.rs/oxoffice-docx) instead.

The provenance variants return `WordLayoutResult`. Its
`body_layout_fragments` accessor reports the point-space block extent on every
occupied page for one zero-based direct body item. The result keeps an empty
fragment slice for preserved body content that does not enter layout.

The legal revision preset is a pre-1.0 Rust source compatibility change.
`LayoutInput` requires `revision_style` in complete struct literals, and
fillable forms add `form_fields` and the result-local `form_paragraphs`
(`None` from the caller). Facade
`RenderOptions` is `#[non_exhaustive]`: start from `RenderOptions::default()`
and use its `with_*` methods or field assignment.
`ParagraphBlock::change_bar: Option<Color>` replaces `has_visible_revision`.
Public `NoteLayout::revision_ranges` now carries
`Vec<(Range<usize>, Color)>`, preserving the bar colour across note fragments.
Use `RevisionStyle::Plain` for the existing default decoration policy.

## Relationship

This crate converts Word-specific semantic input into the shared positioned
model from `oxoffice-layout`. PDF and raster backends consume that model. It does
not emit PDF or pixels itself.

## Example

```rust,no_run
use oxoffice_docx_layout::{LayoutInput, Result, layout_document_deterministic};

fn page_count(input: &LayoutInput) -> Result<usize> {
    Ok(layout_document_deterministic(input)?.pages.len())
}
```

```toml
[dependencies]
oxoffice-docx-layout = "0.1.0"
```
