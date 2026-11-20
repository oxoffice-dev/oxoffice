# oxoffice-pptx-oxml

`oxoffice-pptx-oxml` provides typed PresentationML parts for schema-level slide work.
It parses and serializes presentations, slides, layouts, masters, shape trees,
notes, comments, diagrams, and timing data.

## Capabilities

- Typed presentation, slide, layout, master, and notes parts.
- Shapes, groups, pictures, connectors, and graphic frames.
- Comments, replies, placeholders, SmartArt, and animation timing structures.
- Part-level round trips with schema-aware ordering and identifier helpers.
- Typed text, tables, charts, media relationships, notes masters, handout
  masters, custom shows, transitions, extensions, and raw preservation state.

## Use it when

Use this crate for schema-level slide, shape, text, relationship, and
presentation-part work. Use the `oxoffice-pptx` facade for complete deck
operations.

## Relationship

`oxoffice-pptx` owns complete package behavior, while layout and rendering crates
consume this typed model. `oxoffice-pptx-oxml` promises part-level parsing and
serialization, not whole-package preservation by itself.

## Example

```rust,no_run
use oxoffice_pptx_oxml::presentation::CT_Presentation;

let presentation = CT_Presentation::from_xml(br#"
  <p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
    <p:notesSz cx="6858000" cy="9144000"/>
  </p:presentation>
"#)?;
assert!(presentation.slide_ids.is_empty());
# Ok::<(), Box<dyn std::error::Error>>(())
```

Add `oxoffice-pptx-oxml = "0.1.0"` to your dependencies. The [API documentation](https://docs.rs/oxoffice-pptx-oxml) lists the modeled PresentationML types.
