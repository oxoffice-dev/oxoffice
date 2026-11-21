# oxoffice-docx-html

`oxoffice-docx-html` converts assembled WordprocessingML content into portable web
text. It produces complete HTML, embeddable fragments, and Markdown directly
from semantic Word content without running pagination.

## Capabilities

- Complete HTML document output and body-only fragments.
- Markdown conversion from the same semantic input.
- Style, numbering, link, and image projection through `HtmlInput`.
- A flow-output path: no pagination runs. List numbers come from the
  numbering counter of `oxoffice-docx-layout`, so they are the numbers layout
  draws.
- Semantic tables, lists, hyperlinks, images, fields, and run formatting lower
  through one owned input model for both complete and fragment output.
- Equations as MathML in HTML and LaTeX in Markdown through an
  `EquationWriter` (the `*_with_equations` functions); `oxoffice-docx`
  supplies its converters. The functions without a writer leave equations
  out.

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
