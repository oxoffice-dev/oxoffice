# oxoffice-docx-py

The Rust bindings behind `oxoffice.docx`, the Word API of the `oxoffice` Python
package. The `oxoffice-py` crate builds the wheel.

`oxoffice.docx` gives Python applications a native DOCX workflow for creating,
opening, editing, comparing, laying out, and rendering Word documents. It
works directly with OOXML packages and does not require Microsoft Word,
LibreOffice, a conversion service, or a Java or .NET runtime.

The package runs the Rust `oxoffice-docx` document engine locally, presents a typed
Python API, and needs no remote service for editing, layout, or rendering.
Review workflows cover comments, tracked revisions, comparison, and table of
contents rebuilding. Complete package saves preserve safe producer XML and
parts that the focused Python surface does not model.

## Installation

Install the current release from PyPI:

```sh
python -m pip install oxoffice
```

`oxoffice` requires CPython 3.10 or newer. Its platform wheels use the Python 3.10
stable ABI, so one wheel supports multiple compatible Python versions. Source
installation requires a Rust toolchain and maturin.

## Quick start

```python
from oxoffice.docx import Document

doc = Document()
doc.add_paragraph("Hello from Python")
doc.save("hello.docx")

pdf = doc.to_pdf()
with open("report.pdf", "wb") as output:
    output.write(pdf)
```

## Capabilities

- File and byte-based DOCX input and output.
- Paragraphs, runs, fonts, tables, rows, cells, sections, and styles.
- Rich per-section headers and footers, related stories, and hyperlinks
  resolved, retargeted, or removed in any story.
- Paragraph text replacement that keeps paragraph formatting, comments, and
  bookmarks.
- Counted literal replacement with `Paragraph.replace_text`, `Cell.replace_text`
  and `Document.replace_text_at`, scoped to the selected owner and preserving
  run formatting. `expect` checks the local count before publishing any edit.
- Paragraph style assignment by style ID or name, checked against the styles
  the document defines.
- New documents with Word's usual styles, such as `Heading 2`, `Title`,
  `List Paragraph`, `Caption`, and `Table Grid`.
- Style creation with a font, spacing, and indentation through
  `Document.add_style`, checked updates through `Document.set_style`, plus
  style removal and default selection.
- Numbering definitions and instances built from `ListLevel` values, and
  paragraph styles linked to a numbering level.
- Core document properties such as title, author, and revision, read and
  written through `Document.core_properties` under python-docx's names.
- Picture replacement and resizing by image relationship.
- Complete paragraph and run formatting, multilingual and vertical typography,
  conditional and floating tables, section semantics, settings, fields, forms,
  equations, drawings, comments, and metadata.
- Tracked comparison, main-body comment threads, bookmarks, revision
  resolution, and TOC insertion and rebuilding.
- Deterministic layout fragments, page geometry, and PDF, PDF/A, SVG, PNG,
  JPEG, and TIFF output through the native document engine, with caller fonts
  or a font directory for PDF. PDF and raster methods accept keyword-only
  `revision_view="tracked"` to show tracked changes, with optional
  `revision_style="legal"` for blue insertions, red deletions and green moves.
  Legal styling requires the tracked view and leaves stored formatting intact.
- Fillable PDF forms: `to_pdf(fillable_forms=True)` and
  `to_pdfa_deterministic(fillable_forms=True)` export the content controls of
  the body as form fields, text, check boxes, lists and dates with their
  values. A control tagged `signature` or `signature:<name>` becomes an empty
  signature field. It needs the accepted view, and
  `to_pdf_with_diagnostics` names each control of the body it leaves static.
- Python collections with negative indexes, slices, iteration, and explicit
  stale-handle errors after structural changes.


## Package signatures

`Document.sign(private_key, certificate)` accepts bytes containing a PKCS#8
RSA private key and X.509 certificate as DER or single unencrypted PEM blocks.
Each input is limited to1MiB. The returned ordinary dictionary reports signer
identity, cryptographic validity, complete package coverage, covered parts,
relationships and precise issue payloads. `verify_signatures()` returns a tuple
of these dictionaries for current content, or an empty tuple when unsigned.
Later edits can invalidate a signature. Verification preserves live handles.
The certificate identity does not establish certificate-chain trust. Existing
native refusals of signed input and external relationships remain in force.

## Use it when

Use `oxoffice.docx` when a Python service, desktop application, or automation task
needs to work with Word documents locally. It is suited to template updates,
document assembly, review workflows, comparison, and deterministic output.

## Relationship

The Python API is intentionally focused. It is source compatible with the
documented `oxoffice.docx` surface, but it is not a drop-in replacement for every
python-docx method or private lxml object.

## Example

Open and update an existing document:

```python
from oxoffice.docx import Document

document = Document("template.docx")
document.add_paragraph("Approved")
document.save("approved.docx")
```

Unlike python-docx, `save` refuses to replace the file the document was opened
from, so an edit never silently overwrites its input. Save under another name,
or pass `overwrite_source=True` to replace it on purpose. The file is written
atomically: a failed save leaves the destination as it was.

## Package XML escape hatch

For an unmodelled package part, edit the DOCX ZIP with lxml, then reopen it
with `oxoffice.docx`. Keep the original ZIP members and update package relationships
and content types if an edit adds or removes a part. For example, to change
an existing application property:

```python
from io import BytesIO
from zipfile import ZipFile
from lxml import etree
from oxoffice.docx import Document

source = Document("report.docx").to_bytes()
output = BytesIO()
with ZipFile(BytesIO(source)) as original, ZipFile(output, "w") as edited:
    for member in original.infolist():
        data = original.read(member.filename)
        if member.filename == "docProps/app.xml":
            root = etree.fromstring(data)
            ns = "http://schemas.openxmlformats.org/officeDocument/2006/extended-properties"
            root.find(f"{{{ns}}}Application").text = "Report service"
            data = etree.tostring(root, xml_declaration=True, encoding="UTF-8")
        edited.writestr(member, data)
document = Document.from_bytes(output.getvalue())
document.save("report-edited.docx")
```

The typed API handles style formatting, comments, fields, relationships and
other modeled operations. `set_style` keeps properties not supplied in the
call and cannot clear an existing theme font or theme colour.

## Type checking

The distribution includes hand-written extension stubs and a `py.typed`
marker. Editors and type checkers can inspect concrete document, paragraph,
run, table, section, story, comment, comparison, and layout types.

```sh
python -m pip install mypy
python -m mypy your_application.py
```

`Document.set_header(text)` and `Document.set_footer(text)` replace a complete
story through one checked transaction. Removing a complete owned comment thread
cleans its definitions and companion metadata. Partial ranges or malformed
ownership raise `DocxError` and preserve the document and live handles.
Successful replacement invalidates earlier handles once.

## Project links

- [Source repository](https://github.com/oxoffice-dev/oxoffice)
- [Issue tracker](https://github.com/oxoffice-dev/oxoffice/issues)
- [Changelog](https://github.com/oxoffice-dev/oxoffice/blob/main/CHANGELOG.md)

## License

Licensed under the
[MIT License](https://github.com/oxoffice-dev/oxoffice/blob/main/LICENSE).

## Licensed font embedding

Use caller-owned font bytes with explicit authorization under their exact
license identity. Binary readers are bounded and consumed through actual EOF.

```python
from pathlib import Path
from oxoffice.docx import Document, FontEmbeddingLicense

value = Document()
license = FontEmbeddingLicense(authorized=True, identity="SIL Open Font License 1.1")
value.embed_font("My Face", Path("MyFace-Regular.ttf"), license=license)
value.save("embedded.docx")
for font in value.fonts:
    print(font)
value.embed_true_type_fonts = False
value.remove_embedded_font("My Face", "regular")
```

Kinds are regular, bold, italic and bold_italic. Snapshots are immutable and
remain readable after later native edits. Flags distinguish None, False and
True, where None removes only the owned setting. Explicit caller render fonts
override matching embedded faces. Unsupported imported PowerPoint formats
retain original bytes with absent decoded data.
