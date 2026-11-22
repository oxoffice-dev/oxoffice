---
date: 2026-11-22
status: accepted
scope: docx
---

# Cut and pasted content keeps its bookmarks whole and its ids unique

## Context

A `ContentFragment` is one direct story item (a paragraph, a table, a block
control) taken by `remove_content_at` (Python `pop_content`) and put back by
`insert_content`. Two defects share it (#349, #456):

- **Identities.** `insert_content` writes the fragment as it is. Inserted
  twice, or while its source is still there, a fragment with a picture or a
  bookmark is refused ("duplicate drawing id 1", "duplicate bookmark id 5");
  a block control's `w:sdt/w:id` and a note reference are written twice
  silently, two references showing one note. `clone_content` renumbers every
  identity, but a plain cut and paste must keep a bookmark's name, or the
  REF fields and links that name it break.
- **Copies.** `clone_content` of a paragraph holding a note reference
  writes a second reference to the same note, and of a Google Docs heading
  is refused ("bookmark ownership is incomplete"): the copy holds the end of
  the heading's bookmark without its start.
- **Bookmarks around the block.** Since #455 a block owns the bookmark
  starts written just before it whose bookmark ends inside it (Google Docs
  writes every heading's bookmark so). `remove_content_at` drops those
  starts, but the fragment cannot carry them: the pasted heading holds the
  end without its start, and the `_Toc` link of the contents breaks. A
  bookmark written around a whole table, start just before it and end just
  after it, is not owned at all: moving or removing the table leaves an
  empty bookmark at the old place.

## Decision

- **A fragment carries the bookmark markers its item owns.** Besides the
  item, a fragment holds the bookmark starts written just before it and the
  bookmark ends written just after it that belong to it. `insert_content`
  writes them around the item. A fragment built from a model
  (`ContentFragment::paragraph`...) carries none.
- **A block owns a bookmark that wraps it exactly.** A leading start whose
  bookmark ends inside the block, or in the ends written just after it,
  belongs to the block, with those ends. `move_content`,
  `remove_content_at`, `remove_content`, `remove_content_range` and
  `clone_content` take them with the block; a start whose bookmark ends
  further on stays.
- **An insertion freshens only what is taken.** `insert_content` keeps every
  identity the destination does not hold, so a cut and paste changes
  nothing, and gives a fresh one to each that it holds: a bookmark id, a
  bookmark name (the REF fields and links of the fragment that name it
  follow; those elsewhere keep naming the bookmark they named), a control
  id, a drawing id. A note reference whose note another reference already
  shows gets a copy of the note, with fresh identities. `clone_content`
  keeps renumbering every identity, and copies the notes its copy
  references the same way (it repeated the reference to one note).
- Not done: a fragment of several items. Ranges keep going through
  `DocumentFragment`.

## Plan

One pull request (#349, #456), tests in `tests/docx/document_merging.rs`:
the same fragment inserted twice reopens with unique ids and two notes; a cut
and paste keeps the bookmark name and its REF; a cut Google heading pasted
keeps its bookmark whole; a table wrapped by a bookmark moves and goes with
it.

## Links

- Issues #349, #456; earlier: #448 part B (#455), #348.
