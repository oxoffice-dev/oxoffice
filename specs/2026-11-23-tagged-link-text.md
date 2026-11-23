---
date: 2026-11-23
status: shipped
scope: pdf
---

# A tagged link holds its text

## Context

Since #453 a tagged PDF puts each link annotation under a `/Link` structure element of the paragraph around it, so
the PDF/UA claim holds. Three gaps are left in #159 for assistive technology:

- **The text is not in the link.** The `/Link` element holds only the annotation (`OBJR`). The marked content of the
  link's text stays a kid of the paragraph, so a screen reader reads the text, then announces a link with no text,
  and falls back to the annotation's `/Contents`. Tagged PDF (ISO 32000-1, 14.8.4.4.2, Link elements) lets a
  `/Link` element hold both the link's content and its annotation, which is what readers announce as the link.
- **One link, several elements.** The layout gives one annotation per piece of a link: a hyperlink over two runs, or
  wrapped over two lines, writes two `/Link` elements, read as two links.
- **`/Contents` is the target.** A tagged link describes itself with its URL, or `#bookmark` for an internal link,
  instead of what the reader sees.

The layout already marks each run of a paragraph with its own marked content sequence (`paginator.rs` wraps every
text element in a `MarkedContent` of the paragraph), and draws each piece of a link as its text run followed by a
`LinkAnnotation` whose rectangle is the box of that run: the run's left edge, the line's top and height.

## Decision

**The writer finds a link's text by geometry, groups the pieces of one link, and writes one `/Link` element per link
holding the marked content of its text and the object reference of each of its annotations.** Everything stays in
`oxoffice-pdf`; the layout's output types do not change.

- **A link's text**: a marked content sequence of the same structure parent as the annotation, on the same page,
  whose text runs all start inside the annotation's rectangle: their origin, in page coordinates, from the left edge
  (half a point of slack) to before the right edge, where the layout starts the next run. A sequence that holds runs
  both inside and outside a link stays the paragraph's: a marked content identifier cannot be split.
- **One link**: consecutive annotations (in page order, then across a page break) with the same parent and the same
  target, with no other text sequence of that parent between their texts, form one link. Two adjacent hyperlinks to
  the same target merge into one; that reads the same. A link outside any structure element (a header or footer
  link) stays one link per annotation.
- **The element**: one `/Link` per link, under the paragraph where its first piece was; its kids, in reading order,
  are the marked content of its text and one `OBJR` per annotation. The parent tree gives the `/Link` element for
  those marked content identifiers and for each annotation's `StructParent`. The text's sequences leave the
  paragraph's kids, which keep the `/Link` element at the place of the first one.
- **`/Contents`** of each annotation of a tagged link: the link's text, its pieces joined (a space between two of
  its lines, told by its annotations' rectangles, unless one is there), or the target when the link has no text (a
  linked picture).
- The writer reads run text only when the document has a link, and looks for a link's text among the sequences of
  its parent only.
- Untagged output does not change.

Not done here: Word cross-references (`REF`/`PAGEREF` `\h`, `HYPERLINK \l`) as links and a destination at the line
where a bookmark starts are the other half of #159, in the paragraph engine and the paginator.

## Plan

One pull request, `oxoffice-pdf` only (`structure.rs`, `writer.rs`), tests in `crates/oxoffice-pdf/tests/pdf/links.rs`:
the text of a link is inside its `/Link` element and out of the paragraph; a link over two runs and over two lines
is one `/Link` with two `OBJR`; two links to different targets stay two; a run outside the rectangle stays the
paragraph's; `/Contents` is the text, or the target for a link without text; the parent tree maps the text's
identifiers to the `/Link` element. A docx test renders a hyperlink wrapped over two lines.

## Links

- Issue #159; earlier: #453 (links tagged under `/Link`), #454.
