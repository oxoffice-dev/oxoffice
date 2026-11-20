# oxoffice-pptx-render

`oxoffice-pptx-render` lowers resolved presentation content into the shared positioned
page model. It assembles source-scoped relationships and media, lays out one
page per slide, and offers deterministic bundled-font output for stable
rendering pipelines.

## Capabilities

- `RenderInput` assembly for slides, media, fonts, and metadata.
- Source-scoped slide, layout, and master relationships.
- Shapes, text, tables, images, groups, and backgrounds lowered to page frames.
- Structured diagnostics and `LayoutResult` output for downstream backends.
- Charts, SmartArt text, hyperlinks, speaker notes, handout composition, and
  animation snapshots share deterministic font and media resolution.

## Use it when

Use this crate when integrating presentation layout below the `oxoffice-pptx` facade.
Applications normally call deterministic output methods on
`oxoffice_pptx::Presentation`.

## Relationship

It consumes `oxoffice-pptx-layout` output and produces `oxoffice-layout` page frames. It
does not write PDF or raster files. Those formats belong to downstream output
backends and the high-level facade.

## Example

```rust,no_run
use oxoffice_pptx_render::{RelScope, RelScopes};

let relationships = RelScopes::default();
let missing = relationships.get(RelScope::Slide, "rId1");
assert!(missing.is_err());
```

Add `oxoffice-pptx-render = "0.1.0"` to your dependencies. See the [rendering API](https://docs.rs/oxoffice-pptx-render) for input and output types.
