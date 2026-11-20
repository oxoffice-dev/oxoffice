# oxoffice-docx-html

`oxoffice-docx-html` converts assembled WordprocessingML content into portable web
text. It produces complete HTML, embeddable fragments, and Markdown directly
from semantic Word content without running pagination.

## Capabilities

- Complete HTML document output and body-only fragments.
- Markdown conversion from the same semantic input.
- Style, numbering, link, and image projection through `HtmlInput`.
- A flow-output path with no page-layout or fixed-output dependency.
- Semantic tables, lists, hyperlinks, images, fields, and run formatting lower
  through one owned input model for both complete and fragment output.

## Use it when

Use this crate when an application already owns `HtmlInput`. Use the high-level
[`oxoffice-docx`](https://docs.rs/oxoffice-docx) facade when starting from a DOCX package or
when HTML import is required.

## Relationship

`oxoffice-docx` prepares the `HtmlInput` consumed here. This conversion path is
independent of pagination and PDF rendering.

## Example

```rust,no_run
use oxoffice_docx_html::{HtmlInput, HtmlOptions, to_html_document, to_markdown};

fn export(input: &HtmlInput) -> (String, String) {
    (
        to_html_document(input, &HtmlOptions::default()),
        to_markdown(input),
    )
}
```

```toml
[dependencies]
oxoffice-docx-html = "0.1.0"
```
