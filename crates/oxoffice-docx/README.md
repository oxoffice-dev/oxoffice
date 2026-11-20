# oxoffice-docx

`oxoffice-docx` gives Rust applications one `Document` for complete
Word-compatible package workflows. Open or create documents, edit content,
retain unmodelled package data, compare versions, and render to PDF, images,
HTML and Markdown.

## Capabilities

- Open, create, validate, and save DOCX, DOCM, DOTX and DOTM packages.
- Edit paragraphs, runs, tables, styles, numbering, sections, headers and
  footers, pictures, charts, fields, content controls and comments.
- List, accept and reject tracked changes, and compare two documents into a
  redline.
- Render pages to PDF, PDF/A and PNG, and convert to HTML, Markdown, ODT, RTF
  and EPUB.
- Encrypt and sign packages through the opt-in `agile-encryption` and
  `digital-signatures` features.
- Preserve unmodelled package content byte for byte.

## Use it when

Use this crate for complete DOCX applications. Choose the lower-level
`oxoffice-docx-oxml` only for schema-level WordprocessingML work.

## Example

```rust,no_run
use oxoffice_docx::Document;

let mut doc = Document::new();
doc.set_title("Report");
doc.add_paragraph("Hello from oxoffice.");
doc.save("report.docx")?;
doc.save_pdf("report.pdf")?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

```toml
[dependencies]
oxoffice-docx = "0.1.0"
```
