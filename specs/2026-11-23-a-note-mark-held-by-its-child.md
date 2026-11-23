---
date: 2026-11-23
status: accepted
scope: docx
---

# A note's custom mark held by the child after its reference

## Context

A note reference with `w:customMarkFollows` shows the child right after it
as its mark: a `w:t`, a `w:sym` or a `w:delText` (#164). The run model keeps
the mark as `custom_mark: Option<String>` on `RunContent::FootnoteRef` /
`EndnoteRef`. A `w:t` mark is that string. A symbol or deleted text mark was
read as a separate item, with an empty mark string, and nothing in the model
said which item held the mark:

- `Paragraph::set_text` and `Run::set_text` kept the reference and dropped
  the symbol as body text; the save then wrote an empty `w:t` as the mark.
- The anchor scan of `Paragraph::set_text` took the next `w:t` of the run,
  after the symbol, as the mark text.
- `ref, <w:t></w:t>, <w:delText>x</w:delText>` was written back without the
  empty `w:t`, so Word then took the deleted text as the mark.
- A started reference (`<w:footnoteReference ...></w:footnoteReference>`)
  was kept raw: no reference in the model, its mark read as body text, while
  the note removal removed both.

## Decision

- The mark rule lives on the run model, without a public type change: an
  empty mark directly followed by a symbol or deleted text is held by that
  item (`CT_R::note_mark_child`), as long as nothing Word does not pass over
  sits between them (whitespace, an XML comment, a processing instruction
  or a rendered page break). The symbol stays a typed `Symbol`, so layout
  draws it as before.
- An empty `w:t` mark stays an empty mark; it becomes an empty `Text` item
  only when a symbol or deleted text follows it, so the writer writes it
  back and that child stays body content. A run of the model reads back
  with the same items.
- `Run::set_text` and `Paragraph::set_text` keep the item that holds a mark
  with its reference.
- A started reference with nothing inside reads as the empty one; one that
  holds anything stays verbatim, in the run reader and the source capture
  alike.

Not done: a hidden raw copy of the mark child (the option recorded on #164):
it would take the symbol out of the typed content that layout draws and that
labels the note.

A mark Word splits into the next run waits for Word's answer (probes in the
pull request); if Word does not take it as the mark, the model already agrees.

## Plan

One pull request: oxml run reader and writer, source capture, facade
`set_text` calls; tests in `text::run` and `tests/docx/notes.rs`.

## Links

- #164, #375, #161.
