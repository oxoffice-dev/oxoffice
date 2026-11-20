# oxoffice-docx-oxml

`oxoffice-docx-oxml` provides typed WordprocessingML parts for tools that need
schema-level control. It accepts namespace aliases, serializes children in
schema order, and retains unmodelled XML alongside typed edits.

## Capabilities

- Document bodies, paragraphs, runs, tables, styles, and numbering.
- Headers, footers, notes, comments, revisions, and content controls.
- Transitional OfficeMath parsing and serialization.
- Prefix-tolerant input, schema-ordered output, and raw preservation state.
- Complete paragraph and run properties, multilingual and vertical text,
  conditional and floating tables, section semantics, settings, and web settings.

## Use it when

Use this crate when a tool already owns the XML model. Most applications should
use the package-preserving [`rdocx`](https://docs.rs/rdocx) facade instead.

## Relationship

`rdocx` owns complete DOCX packages and uses these types for WordprocessingML
parts. The shared format-neutral `oxoffice-*` crates remain format-neutral.

## Example

```rust,no_run
use oxoffice_docx_oxml::{BodyContent, CT_Document, CT_P};

let mut document = CT_Document::new();
let mut paragraph = CT_P::new();
paragraph.add_run("Low-level WordprocessingML");
document.body.content.push(BodyContent::Paragraph(Box::new(paragraph)));

assert_eq!(document.body.paragraphs().count(), 1);
```

```toml
[dependencies]
oxoffice-docx-oxml = "0.17.0"
```
