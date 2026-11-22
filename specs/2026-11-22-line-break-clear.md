---
date: 2026-11-22
status: shipped
scope: docx
---

# A line break keeps where its text restarts

## Context

A text-wrapping break can say where the text after it restarts beside a floating object:
`<w:br w:type="textWrapping" w:clear="all"/>` (`ST_BrClear`: `none`, `left`, `right`, `all`). Word writes it
when a break moves text below a picture or a text box that floats beside it.

oxoffice drops `w:clear` (issue #465):

- The three readers of `w:br` (a run, the result of a simple field, the result of a complex field) keep only
  `w:type`, as `BreakType::Line`, `Page` or `Column`. `clear` is the only other attribute of `CT_Br`.
- An edit that rewrites the part writes `<w:br/>`: the next opening, in Word as in oxoffice, puts the text after
  the break beside the floating object.
- The render already puts it there: the paginator re-breaks a paragraph around the floats on its band
  (`reflow_around_wraps`) and knows no clearing break.

## Decision

**The model keeps `w:clear` on a line break, the writer writes it back, and the render restarts the text below the
floats on that side.**

- **Model**: `RunContent::Break(BreakType)` becomes `RunContent::Break(BreakType, BreakClear)`, with `BreakClear`
  `None` (the default, absent or `none`), `Left`, `Right` and `All`. The clear sits beside the kind, not inside
  `BreakType::Line`, so that a page or column break keeps it too (below). The 56 sites that match
  `RunContent::Break` change. The changed types (`RunContent`, `BreakType`, and `InlineItem` and
  `ForcedBreakKind` of `oxoffice-layout` for the render) are not exposed by the public `oxoffice-docx` facade and
  no crate is published to crates.io, so the commit has no `!` marker; the pull request says so in one line.
- **Readers**: the three readers of `w:br` read `w:clear` the same way; an unknown value reads as `None`, as an
  unknown `w:type` reads as a line break. The comparison residual counts `clear` as modelled.
- **Writer**: a line break writes `w:type="textWrapping"` and `w:clear` when its clear is not `None`, Word's own
  spelling; a plain line break stays `<w:br/>`. An explicit `w:clear="none"` is written as absent, as an explicit
  `w:type="textWrapping"` is today: the same value.
- **Page and column breaks**: `w:clear` has a meaning for a text-wrapping break only, but a file that has it on a
  page or column break gets it back (rule 7); the render ignores it there.
- **Render**: the layout's forced line break carries the clear (`InlineItem::LineBreak` and
  `ForcedBreakKind::Line` gain it, in `oxoffice-layout`). When `reflow_around_wraps` re-breaks a paragraph, the line
  after a clearing break starts below the bottom of every float that wraps text (square, tight, through) on that
  side of the line's band: `left` a float whose keep-out area reaches the left text edge, `right` the right one,
  `all` any. A line with no such float beside it does not move. A paragraph with no float on its band is laid out
  as today.
- **Exports**: HTML writes `<br style="clear: left|right|both">`; RTF writes `\lbr1`, `\lbr2`, `\lbr3`. ODT and EPUB
  have no equivalent and write a plain line break, as today.
- **Facade**: no new call. The clear reaches the facade through the model; text, Markdown and the CLI read a
  clearing break as a line break, as today.

**What we do not do**:

- A clearing break in a table cell or a text box still moves below the floats of the page's body only (the floats
  the paginator places); floats inside a cell are not wrap obstacles today.
- A body paragraph is re-broken once, where it starts. When the paginator then moves it to the next page or splits
  it, the gap a clearing break opened travels with its line, as the widths reserved beside the floats already do.
- A field result is drawn from its result text, where a break is a plain line break: the clear a field result keeps
  is written back, not rendered.

## Plan

One pull request, after the on/off and XML escape pull requests of the same run (it touches the files many other
themes touch). It says whether render baselines or corpus page counts move, and asks for a Word check. Tests:

- `text_and_runs`: a run, a simple field result and a complex field result with `clear="all"`, `left` and `right`
  keep it through an edit of another paragraph and a reopen, on a line, a page and a column break; `clear="none"`
  and an unknown value write `<w:br/>`.
- `render`: a paragraph beside a square-wrapped picture on the left, with a clearing break: the line after the
  break starts below the picture with `left` and `all`, and stays beside it with `right` and `none`.
- `html_and_markdown`: the HTML export writes the clear.

## Links

- Issue #465, found by the review of #434.
- Backlog #477, theme XML.
