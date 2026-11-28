# oxoffice-drawing

Parse, edit, resolve, and serialize reusable DrawingML content without
depending on a document format.

## Capabilities

- Typed colors with theme and color-map resolution; the SpreadsheetML tint
  of a cell style colour (`apply_hls_tint`).
- Percentages read in their Transitional (`50000`) and Strict (`50%`) forms,
  written in the Transitional one.
- Solid, gradient, pattern, image, and no-fill models.
- Preset and custom geometry, lines, transforms, text bodies, and tables.
- Schema-ordered writing with ordered preservation of unmodelled children.
- Shared chart, table, color, theme, and geometry models serve both document
  families without importing either facade.

## Use it when

Use this crate when reading or writing DrawingML shared by DOCX and PPTX packages. Use `oxoffice-pptx` or `oxoffice-docx` for complete documents.

## Relationship

It consumes format-neutral OOXML primitives and supplies drawing models to
presentation and rendering crates. Format-specific anchors, wrappers,
packaging, and rendering belong elsewhere. Effects and DrawingML elements that
are not typed remain preserved rather than being interpreted.

## Example

```rust,no_run
use oxoffice_drawing::fill::Fill;

let fill = Fill::from_xml(
    br#"<a:noFill xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"/>"#,
)?;
assert!(matches!(fill, Fill::NoFill(_)));
# Ok::<(), oxoffice_drawing::fill::FillError>(())
```

Add `oxoffice-drawing = "0.1.0"` to your dependencies. Browse the [typed DrawingML API](https://docs.rs/oxoffice-drawing) before constructing schema-level values directly.
