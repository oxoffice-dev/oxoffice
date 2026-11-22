---
date: 2026-11-22
status: shipped
scope: perf
---

# PowerPoint binding: owners that do not depend on their slide's position

## Context

In Python, `SlideCollection.move` and `SlideCollection.add_slide_at` cost
more as the deck grows (#98). The Rust half is fixed: `add_slide_at` no
longer re-reads the whole package (#479). What is left is in the binding
(`crates/oxoffice-pptx-py/src/presentation.rs`).

The binding keeps an inventory of owners, one per shape, text body,
paragraph, run, table cell and so on, in parts: one part per slide, plus one
for the layouts and masters. An owner's address is a path whose first
segment is its slide's index (`PathSeg::Slide(i)`). Three indexes hang on
that index:

- `PartOwners::by_path` is keyed by the full path;
- `OwnerInventory::by_generation` maps each owner to `(OwnerPart::Slide(i), n)`;
- `PartOwners::moved_to(i)` therefore clones every owner of a part whose slide
  moves, rewrites the first segment of every path, and hashes them all again.
  `OwnerInventory::apply` then re-inserts every owner of every moved part
  into `by_generation`.

Moving the last slide to the front, or inserting a slide there, shifts every
slide, so it pays for every owner of the deck. Heavy deck of
`test_pptx_scaling.py` (25 text boxes, 12 shapes, a 4x4 table, a picture and
notes per slide), `scaling` wheel, Linux, with #479 applied:

| Slides | `move(last, 0)` | `add_slide_at(blank, 0)` | `add_slide` |
| --- | --- | --- | --- |
| 5 | 0.89 ms | 1.28 ms | 0.44 ms |
| 20 | 3.43 ms | 3.82 ms | 0.55 ms |
| 80 | 13.31 ms | 14.00 ms | 0.94 ms |

That is about 165 µs per shifted slide on that deck, against 9 µs on a deck
with one title per slide. The cost follows the shapes of the shifted slides,
not the slide list.

## Decision

A part's owners no longer carry their slide's position, so a moved part is
shared, not rebuilt.

- **Paths relative to their part.** A slide part stores its owners' paths
  without the leading `Slide(i)`. The part's position is where it sits in
  `OwnerParts::slides`. `by_path` is keyed by the relative path. The full
  path is rebuilt on read: `OwnerInventory::owner(generation)` and
  `OwnerParts::owner_at(path)` return a small view (the owner and its slide
  position) whose `path()` puts the segment back. The layout and master part
  keeps its paths as they are (they do not start with a slide).
- **A stable key per part.** Each `PartOwners` gets a key that never
  changes, allocated when the part is built. `by_generation` maps an owner to
  `(key, n)`. `OwnerParts` keeps the position of each key, rebuilt when the
  slide list changes: one entry per slide, the cost of the slide list
  itself.
- **A moved part is shared.** The owners, `by_path` and `shape_ids` of a part
  sit behind one `Arc`. `moved_to` becomes a pointer copy, and `apply` no
  longer touches `by_generation` for moved parts.

The survival proofs, the revision tokens and what a Python handle sees do not
change: every reader goes through the view and gets the same full path as
today. The debug cross-checks stay (`assert_same_preparation`,
`assert_indexes`) and check the new indexes against a full walk.

Not done here:

- The layouts and masters part keeps its paths: removing a layout re-addresses
  the layouts after it, which is rare and bounded by the layout count.
- A structural edit still copies the slide part list and rebuilds the key
  positions, O(slides), like the Rust slide list. Every call also pays about
  2 µs per slide for the candidate copy (one pointer per part, #479). That
  residual is accepted.
- No public API change in Rust, the CLI or Python.

## Plan

One pull request, `perf(python)`, closes #98.

1. Split `PartOwners` into a shared body (owners, `by_path`, `shape_ids`,
   refusal) and its key. Store relative paths, add the view, and move every
   reader of `address.path` to it. About 20 sites in `presentation.rs`.
2. `by_generation` by key; `OwnerParts` keeps key positions; `moved_to` and
   `apply` share moved parts.
3. Check:
   - the Python tests under debug assertions (`maturin develop`, `pytest -n 8`
     on `crates/oxoffice-py/tests`), where the cross-checks compare every
     prepared edit against a full walk;
   - `test_pptx_scaling.py` at 16N for the two calls;
   - the table above measured again on the heavy deck and on the light one;
   - proof that the slope no longer follows the shapes: the slope per slide
     of `move` must be about the same on both decks.
4. A guard for the regression: `test_pptx_scaling.py` gets a larger deck for
   `move` and `add_slide_at` only (16N), where the old cost fails the
   2.5x band. The decks of the other calls stay 4N.

Estimate: about three hours, most of it the readers of step 1 and the checks.
Risk: medium, local to the binding. A wrong position would hand a Python
handle the wrong slide, and the debug cross-checks catch that on every
binding test.

## Outcome

Shipped as planned, with one simplification: a slide part was already
behind an `Arc` (`OwnerParts::slides`), so the key lives in `PartOwners`
itself rather than beside a separate body. `Arc::make_mut` keeps the key of
the part it copies, so an in-place edit of a part keeps its index entries
valid. Stored owners hold a `PartAddress` (path within the part); walked
addresses keep whole paths, and `OwnerView` gives a stored owner its whole
path back. `PartOwners::moved_to` is gone. `OwnerInventory::assert_indexes`
also checks the key positions against the slide list.

Same machine and deck as the table above, `scaling` wheels of main and of
this change, fastest of 41 calls:

| Slides | `move`, heavy | `move`, light | `add_slide_at`, heavy | `add_slide_at`, light |
| --- | --- | --- | --- | --- |
| 5 | 0.91 -> 0.08 ms | 0.11 -> 0.07 ms | 1.32 -> 0.47 ms | 0.51 -> 0.52 ms |
| 20 | 3.96 -> 0.12 ms | 0.24 -> 0.10 ms | 4.02 -> 0.60 ms | 0.68 -> 0.58 ms |
| 80 | 16.46 -> 0.30 ms | 0.76 -> 0.28 ms | 18.87 -> 1.30 ms | 1.40 -> 0.97 ms |

The slope per slide of `move` is now 2.9 µs on the heavy deck and 2.7 µs
on the light one (207 µs and 8.6 µs before): it no longer follows the
shapes. `add_slide_at` keeps 11 µs per slide on the heavy deck against 6 µs
on the light one: that is the slope of `add_slide` on the same decks
(about 7 µs and 4 µs, unchanged here) plus the shift.

`test_pptx_scaling.py` times `move` and `add_slide_at` at N and 16N
(`SLIDE_ORDER`); main fails it (move 13.7 ms at 80 slides against 1.0 ms
at 5), this change passes. `SlideCollection.move` joins the QUICK set, so
every pull request runs the guard (the 16N deck takes about 5 s to build).
`test_pptx_slide_handles.py` checks that handles taken before slides move,
are inserted, removed or duplicated still read and edit their own slide.

## Links

- #98, #479 (the Rust half), #478 (signature marker, unrelated)
- [Edit cost at the root](2026-11-21-edit-cost-at-the-root.md), the same
  principle for docx: an edit costs its own element
