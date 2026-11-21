---
date: 2026-11-21
status: draft
scope: tests
---

# Consumer workflows: the suite checks what a first user does

## Context

oxoffice has its first downstream user: agent tools that maintain Word and
PowerPoint files for one maintainer, through the Python package and the CLI
only, on rdocx 0.17.0 and rpptx 0.15.0 today, the code oxoffice was imported
from (85c3b85). They will switch as soon as oxoffice is ready, and the switch
has to work the first time. Their files are edited in place in a synced
folder, and people open the same files in Word, Google Docs and Google Slides
between two sessions:

- **a long report and its companion documents**: versioned, rendered to PDF at
  each version, redlined between versions; up to about 40 pages and 1.5 MB,
  a TOC field in a content control, two footer parts, figures as PNG with an
  SVG twin, often saved last by Google Docs;
- **records co-edited in Word (French interface) and Google Docs**: up to
  about 45 pages, 30 tables and 30 content controls, localised style ids,
  comments and tracked changes by named people;
- **decks** edited in Google Slides: text boxes only, one font, no autofit.

Their maintainer wrote down what the tools rely on: 13 workflows, 12
invariants and 14 past breakages, each with a neutral reproduction. That list
is private input; this spec restates it in its own words. Items keep the
list's labels (W1 to W13, I1 to I12, R1 to R14) so that both sides can map
them. A fourteenth workflow, on spreadsheets, waits for the spreadsheet facade (ROADMAP).

What the suite has, on `main` 83665a7:

- **Behaviour**: 1,029 docx tests in `--test docx` (31 modules, 35 strict
  known failures), 277 pptx tests in `--test pptx`, 80 Python tests, 28 CLI
  tests, the skill examples on `main`.
- **Corpus**: 64 files in 4.4 MiB (16 families saved by Word, PowerPoint,
  Google and LibreOffice), every file round-tripped byte for byte. The Word
  copies come from a Word in French: they carry `Titre1`, `Policepardfaut`,
  `En-tte` and an unused linked pair `Titre7` / `Titre7Car`.
- **Render layer A**: a baseline of every page of every docx corpus file
  (page count, pixel and content hashes), 40 files and 209 pages on pull
  requests; pptx in #328.
- **CI**, measured on run 44903342880 (a pull request) and 44902746773
  (`main`): the Linux build job takes 15 min 21 s on a pull request, of which
  7 min 29 s for the Rust integration tests and 5 min 57 s for the Python
  tests; Windows 14 min 17 s. On `main`, 4 min 22 s and 3 min 09 s.

Read against that suite, most items are partly covered: the calls exist and
the behaviour modules test them one at a time, mostly on synthetic
documents, but no test walks a workflow on a producer's file, the CLI JSON
keys are pinned nowhere, the deck text layout has no test at all, and
nothing tells the user what changed since rdocx 0.17.0. The mapping also
found four bugs (#340 to #343).

## Decision

1. **Fold every case into the existing layers and modules.** No new test
   binary, no new CI job, no parallel suite. A workflow becomes one walk on a
   corpus file, in the module of its main area, reading back from the saved
   package; the calls the behaviour modules already check are pointed to, not
   tested again.
2. **One file on pull requests, every copy on `main`.** A workflow test runs
   on one corpus file on a pull request and on every producer's copy when
   `OXOFFICE_WORKFLOWS=full`, set on the existing Rust and Python test steps
   for pushes to `main` and the nightly run (one line in `ci.yml`, with the CI
   session). Budget: **about 60 s more on the Linux build job of a pull
   request for the whole plan**, and at most 3 minutes more on `main`. Job
   timings vary by minutes between runs of the same tree, so each pull
   request measures the tests it adds on their own (`cargo test` and pytest
   `--durations`, PR set and `full` set) and states both in its body.
3. **What breaks** becomes an issue and a strict known failure
   (`#[should_panic(expected = ...)]`, or `xfail(strict=True)` in Python),
   and a limitation entry when users can work around it. A walk over every
   producer's copy cannot carry one `should_panic` for one bad copy: its
   known failures per file live in a list checked both ways, like
   `known_failure` in the corpus manifest and `tests/render/known.toml`
   (`tests/corpus/workflows-known.toml`: file, workflow, issue), so a fix
   flips the test until the line goes. Open issues a
   workflow meets are listed below, so that their fixes come first.
4. **A migration note**, `docs/MIGRATING-FROM-RDOCX.md`, lists every name,
   flag, key, default and refusal that differs from rdocx 0.17.0 / rpptx
   0.15.0, with what to write instead. A Python test fails when a public name
   of rdocx 0.17.0 / rpptx 0.15.0 is neither in oxoffice nor in the note.
   No compatibility aliases.
5. **Three corpus families**, authored by `tests/corpus/generate/` and saved
   by the real producers, for the traits the corpus lacks: a versioned report
   and its second version, a record saved by Word then Google Docs, a deck of
   text boxes. About 300 KB a copy, under 5 MB in all (the corpus stays under
   9 MB of its 20). They join render layer A like any corpus file: their
   pages are hashed, no picture is committed, so the "golden" TOC page and
   slide cost no image.
6. **Kept as oxoffice decided**, written in the migration note rather than
   changed: the package and CLI names; `save` refusing its source; new
   comments dated unless `undated=True`; `replace_all` and `docx replace`
   refusing text left out of reach; search reading a tab as `\t` and a break
   as `\n`; run text including simple field results; header and footer
   `PAGE` / `NUMPAGES` caches kept by the layout-backed update, as Word keeps
   them (unchanged since 0.17.0); duplicate drawing ids kept as the source had
   them (byte for byte wins), new ones never repeating; tracked changes made
   only by `compare` (unchanged); `docx diff` comparing text, not formatting
   (unchanged); `pptx diff` in plain text (unchanged); an SVG picture of a
   deck refusing a new image.

Not in this spec: spreadsheets; Word pagination beyond page counts and TOC
pages (render layer B compares with Word); Word or PowerPoint in CI (the Mac
runs on demand, as today).

## Map

Status: **covered**, **partly**, **missing**, **different** (oxoffice
decided otherwise, see the migration note). Module paths are under
`crates/oxoffice-docx/tests/docx/` and `crates/oxoffice-pptx/tests/pptx/`
unless named; PR numbers refer to the plan below.

### Invariants

| | Property | Status | What covers it | Gap, and where it goes |
| --- | --- | --- | --- | --- |
| I1 | Open and save without an edit keeps every part byte for byte; after an edit, untouched parts byte for byte, unmodelled XML kept | covered without an edit, partly after | `corpus::every_document_saves_every_part_unchanged` (every docx); edits keeping producer parts in `comments`, `properties_and_settings`, `fonts`, `paragraphs_and_styles` | One edit on every corpus file, then every other part byte-equal: `corpus.rs` (PR 5) |
| I2 | Same input, same calls, same build: same bytes, page count, layout and pixels | partly | corpus round trip; render baselines (hashes committed, so across processes; across platforms at night); `pdf::the_same_document_converts_to_the_same_bytes` | The same edit sequence in two processes gives the same bytes, page count, layout JSON and PDF bytes: Python, `main` (PR 7) |
| I3 | Every text read shows the accepted view; a replacement inside a tracked insertion stays tracked | partly | `revisions_and_compare::the_text_of_a_document_shows_insertions_and_hides_deletions`, `story_paragraphs::tracked_changes_read_in_the_accepted_view` | A replacement wholly inside `w:ins` stays inside it after a save (PR 5) |
| I4 | Files Google Docs saved open, read and edit (decimal measurements, duplicate drawing ids, packed fields, duplicate and multiple default styles, `w:val="0"`, default namespaces) | partly | the 11 Google copies (decimal `tblW`, `w:col`, `w:spacing`; duplicate `docPr`; packed `PAGE` / `NUMPAGES`; default namespaces) through the round trip and the render | A synthetic matrix, one attribute at a time (PR 4); #340 (Google's TOC bookmarks) |
| I5 | Localised style ids are found and assigned by the id the file uses | covered | `fields_and_toc::rebuilding_the_toc_of_a_word_file_saved_in_french_finds_its_headings`, `paragraphs_and_styles::headings_are_found_in_the_word_and_google_reports_alike`, `comments::a_comment_added_to_a_localized_word_file_uses_its_comment_styles` | Kept by the workflow walks on the French Word copies. Open: #169, #144 |
| I6 | Refuse rather than write something wrong; a batch is all or nothing; the input is unchanged on refusal | covered | `text_and_runs::counted_replacement_changes_nothing_when_a_count_is_wrong`, `checked_replacement`, `save::saving_over_the_opened_file_is_refused`, CLI `docx_replace_reach` | CLI `replace --map` with a wrong count writes nothing (PR 3). Different: fields the update cannot compute keep their cache with a diagnostic, a PDF that drops content comes with diagnostics |
| I7 | CLI JSON keys stay, or change in a release note | partly | `docx_stories` (`text --json`, `count --json`), `docx_comment_dates` (`comment list`, `comment add`), `pptx_duplicate_comments` (`pptx comment list`), `pptx_shapes` (`pptx shapes`) read the keys they need; `check_outputs.py` (`main`) a few more; no test pins a whole key set, none reads `diff`, `revision list` or `layout` | A keys-only snapshot per command, in those files (PR 3) |
| I8 | Page numbers from the layout are close to Word's and stable within a release | partly | page counts pinned implicitly by the render baselines | Word's page counts and TOC pages: render layer B, and the versioned report's Word copy (PR 13). A changelog rule for layout moves (PR 2). Open: #156 |
| I9 | Bytes in, bytes out, without touching the source | partly | `save::saving_elsewhere_leaves_the_source_as_it_was`, `test_save.py`, the corpus round trip | `save(other)` equals `to_bytes()`, `open` equals `from_bytes` (PR 5) |
| I10 | A value a call accepted is still accepted, or the change is in the migration note | partly | unchanged since 0.17.0, untested | A parity test of `0`, `Pt(0)`, `Twips(0)`, `Cm(0)` on every length setter (PR 9); #343 |
| I11 | No network, LibreOffice or Office at run time; bundled fonts | partly | the render tests use the bundled fonts; their set is pinned by the resources hash | Arial drawn with Liberation Sans, asserted (PR 7) |
| I12 | No slowdown against rdocx 0.17.0 on these sizes | missing | scaling tests compare N with 16N only | A budget table on `main` against a committed one-off measurement of rdocx 0.17.0 on the same runner, reported, not gating (PR 14). Open: #5, #6, #234, #235 |

### Workflows

| | Workflow | Status | What covers it | Gap, and where it goes |
| --- | --- | --- | --- | --- |
| W1 | Read-only check of a document set: text of every story, runs and fonts, tables, comments, revisions, links, pictures, layout and page count, field instructions, validation, CLI `text`, `layout`, `revision list`, `validate` | partly | `story_paragraphs::every_story_of_the_corpus_reports_is_listed_with_its_kind_part_and_style`, `comments::every_producer_reports_the_corpus_comment_as_it_saved_it`, `revisions_and_compare::each_kind_of_tracked_change_is_read_with_its_author_and_date`, `inspection` (links, pictures, validation) | One read walk on each family copy, counts compared with the authored copy, producer differences pinned (PR 13); CLI `layout` and `revision list` keys (PR 3). Open: #146 |
| W2 | Diff a file against its last version, content and form | partly, different for form | no test of `docx diff`; comments and resolved state through the API | `docx diff --json` of the authored and Google copies: no text difference beyond a pinned list; one added comment shows as one row (PR 3). Different: `diff` compares text; form is compared from `text --json` runs |
| W3 | Counted edit in place, checked part by part before the write | partly | `text_and_runs` (replacements across runs, headers, footers, cells), `checked_replacement`, `paragraphs_and_styles::content_is_removed_by_index_or_by_half_open_range`, `tables::clone_table_row_copies_a_row_with_its_formatting_to_the_requested_place` | One guarded walk on a corpus file: ten counted pairs across body, a cell, first-page and default footers and a tracked insertion, a cloned paragraph, a removed range, then the guard (part set, untouched parts byte for byte, style, list level and runs of unchanged paragraphs); paragraph-scoped replacement (PR 5). Open: #30, #172 |
| W4 | Page numbers from the layout: TOC to the field's depth, page fields, idempotent | partly, different for headers and footers | `fields_and_toc::rebuilding_a_word_toc_reuses_its_bookmarks_and_follows_its_switches`, `rebuilding_a_toc_follows_renamed_added_and_removed_headings`, `page_fields_take_their_numbers_from_the_layout_except_in_a_footer` | On the thesis copies: rebuild and update twice, the second run changes no byte, entries equal the headings to the depth in `TOC n` styles on a dotted right tab, header and footer caches kept in every variant (PR 6); #340 as a strict known failure. Open: #66, #156 |
| W5 | Render a version to PDF | partly | `render::every_page_of_a_corpus_render_has_a_page_size_of_its_document` (PDF pages equal layout pages), `pdf::the_same_document_converts_to_the_same_bytes` | PDF page count equals `page_count()` and bytes equal from two processes (PR 7). Different: content the renderer drops gives a PDF and a diagnostic, not a refusal |
| W6 | Redline two versions, accept and reject round trip, PDF in legal colours | partly | `revisions_and_compare::word_and_character_granularity_mark_only_what_changed`, `a_redline_of_two_word_saved_reports_accepts_to_the_edited_copy`, `renamed_toc_heading_bookmarks_do_not_block_the_comparison`, `swapped_footnote_references_are_compared` | `ignore_fields`, a document with content controls, legal colours in body and footer, versions whose TOC oxoffice rebuilt (PR 7); the family's two versions (PR 13). Open: #66, #68, #120 |
| W7 | Replace a figure keeping its size, SVG twin included | partly | `pictures::replacing_the_figure_of_a_report_keeps_its_size_and_caption`, `resizing_by_relationship_changes_every_picture_that_shows_it` | Only the media parts change; the SVG twin follows; a picture shared with a header (PR 9) |
| W8 | Carry review comments from a Google-saved copy to a clean copy | partly | `comments::moving_a_comment_to_other_text_brings_its_replies`, `a_reply_is_linked_to_its_parent_and_covers_the_same_text` | Threads read in one document and written into another, read back with authors, dates, parents and resolved state (PR 8). Open: #182 |
| W9 | Comments and tracked changes in a named author's name, others' left alone | partly | `comment_on_text::tables_only_counts_the_matches_of_table_cells_only`, `revisions_and_compare::one_revision_is_accepted_or_rejected_by_its_id` | On the contract copies: a redline and five comments by one author, accept and reject by id; the other authors' revisions and threads unchanged (PR 8) |
| W10 | Apply a house style to a record, idempotent | partly | `paragraphs_and_styles`, `lists::restart_numbering_gives_one_item_a_new_start_and_the_items_after_follow`, `tables::cell_shading_borders_margins_and_alignment_survive_a_save`, `test_table_properties.py` | A minimal restyle twice on the French Word report: the second run changes no byte, comments unchanged (PR 9); #342. Open: #82, #91, #234, #235 |
| W11 | A short document from scratch | covered | the behaviour modules | Opening in Word without repair stays on the Mac, on demand |
| W12 | Deck checks (text fit at 100 % and 95 % of a frame, shapes, ids, transitions) and touch-ups | partly; text layout missing | `shapes_walk`, `shapes_and_text::editing_text_keeps_the_formatting_of_the_run_it_replaces`, `replace_text_counts_every_match_across_slides_groups_and_tables`, `pictures::replacing_the_image_keeps_the_picture_geometry_name_and_alt_text`; renders in #328 | `text_layout`, a rounded rectangle's inset, the 95 % factor, overflow, line breaks, transitions, duplicate shape ids, arrowheads (PR 10); a pinned layout table on the deck copies (PR 13) |
| W13 | Text for agents: JSON with run formatting, Markdown | partly | `docx_stories`, `test_story_paragraphs.py`, `html_and_markdown` | Keys of `text --json` (PR 3); Markdown of a record copy keeps headings, lists and tables (PR 13) |

### Past breakages

| | What broke | Status on `main` | Test, and where it goes |
| --- | --- | --- | --- |
| R1 | Google-saved files refused for decimal measurements and duplicate drawing ids | holds on the corpus; duplicate ids different (kept) | Synthetic matrix: `gridCol`, `ind`, `trHeight`, `spacing`, `pgMar`, `pgSz`, `tcW`, `tblW`, two `docPr id="0"` (PR 4) |
| R2 | A rebuilt TOC one page earlier than Word's | not measurable: no copy holds Word's own TOC pages | The versioned report's Word copy, its TOC updated by Word, against `rebuild_toc` (PR 13) |
| R3 | TOC with `\u` refused next to a vertically merged cell | holds (synthetic probe) | Strict test in `fields_and_toc.rs` (PR 6) |
| R4 | TOC refused over a heading deleted with tracked changes | holds (synthetic probe) | Strict test: the deleted heading left out, revisions untouched (PR 6) |
| R5 | A bare `0` length refused after an upgrade | holds, except `set_default_paragraph_format` | Parity test (PR 9); #343 |
| R6 | An unused linked style pair cannot be removed | breaks | Strict known failure on `word/report.docx` (PR 9); #342 |
| R7 | Removing a paragraph orphaned its comment; anchor text, moving a comment, paragraph-scoped replace | holds; paragraph-scoped replace untested | `comments::deleting_commented_content_takes_whole_threads_and_refuses_half_ones`; add no orphan marker in the saved XML and `Paragraph.replace_text(expect=)` (PR 5, PR 8) |
| R8 | Comparison refused renumbered `_Toc` bookmarks and moved footnote references | holds with Word-style fields; breaks on oxoffice's own rebuilt TOC (`PAGEREF` as simple fields) | Strict known failure on #66 with two versions rebuilt by `rebuild_toc` (PR 7) |
| R9 | Threads and resolved state keyed on a comment's first paragraph; a newline in a comment | holds; tests would not see a regression (reads accept any paragraph id) | XML pins on the last paragraph id, resolve on a two-paragraph comment, a Google-form fixture (PR 8) |
| R10 | Run shading not drawn | breaks | Pixel probe in each run's box, strict known failure (PR 7); #341 |
| R11 | Replacement through Python 2 to 4 times the CLI | open | Reported ratio in the budget table (PR 14); #5 |
| R12 | A release with a broken TOC; `NUMPAGES` caches | holds | The W4 walk on every copy on `main` (PR 6) |
| R13 | A text layout change in decks moved every height silently | no pin | The deck layout table (PR 10, PR 13) and the changelog rule (PR 2) |
| R14 | Slide transition not readable | holds, untested | Read on the pitch copies, none on the deck, LibreOffice's empty element decided (PR 10) |

### Names, CLI and release

- **Python names**: of the 188 names the tools use, 187 exist on the same
  class; `RdocxError` is `DocxError` (and `RpptxError` `PptxError`). No class
  or member was removed since the import, no parameter reordered; every
  change adds a keyword-only parameter. The behaviour changes are the ones
  in Decision 6, plus `insert_document` keeping the destination's formatting,
  property and settings setters refusing characters XML cannot carry, core
  property dates of decks as `datetime` (#331), Python 3.10 or later.
- **Helpers** of the skills: in the library since the helpers spec
  (`replace_all` with counts, `save`, `Paragraph.find_text`,
  `Document.isolate_span`, `Document.count_text`, `add_comment_on_text`,
  `Comment.anchor_text`, `story_paragraphs`); the comparison now ignores
  renumbered `_Toc` bookmarks, so the bookmark-stripping workaround goes.
- **CLI**: `rdocx X` is `oxoffice docx X`, `rpptx X` is `oxoffice pptx X`; no
  command or flag removed; keys of `text`, `diff`, `revision list`, `layout`,
  `comment list`, `compare` and the pptx commands unchanged since the import
  except `docx text --json` (story items gain `style`, run text includes
  simple field results). Worth stating in the note: `revision list` says
  `timestamp`, `comment list` says `date`; story kinds are spelled
  `table_cell` in `text` and `table-cell` in `revision list`; `story` is a
  string in `diff` and an object in `revision list`.
- **Release**: 0.1.0 already ships wheels for linux-x86_64, linux-aarch64,
  macOS arm64 and x86_64, Windows, and `SHA256SUMS`; the release that
  carries this plan needs nothing new.

### Open issues a workflow meets

Their fixes come first: #66, #156, #146, #30, #172, #5, #6, #82, #91,
#169, #144, #120, #68, #234, #235, and the new #340 to
#343.

## Plan

Each pull request: the spec section it implements, its tests read back from
the saved package, its strict known failures with their issues, the time of
the tests it adds (PR set and `full` set), one
independent review on the local commit before the push.

| PR | Content | Modules | Runs | Budget |
| --- | --- | --- | --- | --- |
| 1 | This spec | `specs/` | — | — |
| 2 | `docs: the migration note from rdocx`: `docs/MIGRATING-FROM-RDOCX.md`; the public names of rdocx 0.17.0 and rpptx 0.15.0 as a committed list (classes, members, keyword arguments, CLI commands and flags, read from the import's stubs and `--help`); the differences as rows of `tests/migration/differences.toml` (old name, new name or behaviour, commit), from which the note's table is written and checked, since a pull request that edits only `docs/` or Markdown runs no test; a check that each listed name is in oxoffice or in a row, and that each row names something real; the changelog rule for layout moves in `docs/DEVELOPMENT.md` | `crates/oxoffice-py/tests/test_migration.py`, `tests/migration/` | PR | 2 s |
| 3 | `test(cli)`: keys-only snapshots of `docx text`, `diff`, `revision list`, `layout`, `comment list`, `count`, `compare --json`, `pptx text`, `fit`, `shapes`, `comment list`; `docx diff` of the authored and Google report (no text difference beyond a pinned list, one added comment = one row); `replace --map` with a wrong count writes nothing; `OXOFFICE_WORKFLOWS` in `ci.yml` (CI session) | existing files of `crates/oxoffice-cli/tests/` | PR, `main` | 5 s |
| 4 | `test(docx)`: what Google Docs writes, one attribute at a time (R1, I4): decimal measurements read to the nearest integer and saved, duplicate drawing ids kept and no new one repeated, duplicated style ids, two default styles of one type, `w:val="0"` toggles | `tables.rs`, `paragraphs_and_styles.rs`, `sections_and_headers.rs` (`pgMar`, `pgSz`), `pictures.rs` (drawing ids) | PR | 2 s |
| 5 | `test(docx)`: the counted, guarded edit (W3, I1, I3, I9, R7): the guarded walk; one edit on every corpus file with every other part unchanged; a replacement inside a tracked insertion; `save(other)` equals `to_bytes()`; paragraph-scoped replacement | `checked_replacement.rs`, `corpus.rs`, `text_and_runs.rs`, `save.rs`; a part-comparison helper in `support.rs` | PR (one file, no `long-*`), `main` | 8 s |
| 6 | `test(docx)`: page numbers from the layout (W4, R3, R4, R12): the thesis walk twice, header and footer caches in every variant, a French "sur" footer, `\u` with a merged cell, a deleted heading, fields it cannot update; #340 strict | `fields_and_toc.rs` | PR (Word thesis), `main` (every copy) | 8 s |
| 7 | `test(docx)`: render and redline (W5, W6, R8, R10, I2, I11): PDF pages equal `page_count()`, two processes same bytes; `ignore_fields`; the form copies compared; legal colours probed in body and footer; R8 through `rebuild_toc` strict on #66; run shading strict on #341; Arial drawn with Liberation Sans | `revisions_and_compare.rs`, `pdf.rs`, `crates/oxoffice-py/tests/test_save.py` | PR, `main` | 8 s |
| 8 | `test(docx)`: review annotations (W8, W9, R7, R9): last-paragraph ids in the XML, resolve on a two-paragraph comment, threads carried between documents, another author's revisions and threads untouched, no orphan comment marker | `comments.rs`, `revisions_and_compare.rs` | PR | 3 s |
| 9 | `test(docx)`: house style, lengths and pictures (W7, W10, R5, R6, I10): restyle twice on the French report; the linked pair strict on #342; length parity strict on #343; media-only part changes, SVG twin, picture shared with a header | `paragraphs_and_styles.rs`, `pictures.rs`, `crates/oxoffice-py/tests/test_lengths.py` | PR | 4 s |
| 10 | `test(pptx)`: deck text layout and checks (W12, R13, R14): `text_layout` fields, rectangle against rounded rectangle, the 95 % factor and the overflow flip, bad factors refused, transitions on the corpus copies, `DuplicateShapeId`, arrowheads; the layout tests gated on the `render` feature like the chart renders (the WebAssembly job builds `--test pptx` without it) | `shapes_and_text.rs`, `slides_and_layouts.rs`, `corpus.rs`; a Python smoke test | PR | 3 s |
| 11 | `test(corpus)`: the three families, authored and saved by LibreOffice; `produce.py` routes for Word then Google (`word-google/`) and for Word updating the TOC fields of the families that ask for it; manifest features for the new traits | `tests/corpus/generate/` | PR (round trip, render) | 10 s |
| 12 | `test(corpus)`: their Word, PowerPoint, Google and Word-then-Google copies, from a run on the Mac | `tests/corpus/files/` | PR | 5 s |
| 13 | `test`: the workflow walks on the families (W1 read walk, W2, W6 two versions, W13 Markdown, R2 TOC pages against Word's, the deck layout table) | the modules above | PR (one file each), `main` | 3 s |
| 14 | `test(python)`: the budget table on `main` (I12, R11): open and page count of a 45-page, 30-table document built by `builders.py`, about 100 counted replacements on the Word long report through Python and the CLI, a read-only walk of the docx corpus, the deck checks; against a one-off measurement of rdocx 0.17.0 and rpptx 0.15.0 from PyPI on the same runner, committed with the table; reported in the job summary, not gating | `crates/oxoffice-py/tests/test_docx_scaling.py`, `test_pptx_scaling.py` | `main` | 0 s on PRs |

Order: 2 and 3 first (the switch needs them), then 4 to 10 in any order
(they touch different modules), 11 and 12 with the Mac session, 13 after 12,
14 last. Pull requests 4 to 10 touch modules OX-D2 also edits: a line to it
before each.

**Families** (PR 11 and 12), neutral content in English with a short French
twin, about 300 KB a copy:

| Family | Saved by | Carries |
| --- | --- | --- |
| `versioned-report` and `versioned-report-2` | authored, LibreOffice, Google, Word (the second version: authored, Google, Word) | a cover with a version label, a history table, a `TOC \o "1-1" \h \z` in a `docPartObj` content control with stale entries, headings 1 to 3 numbered by a list plus one unnumbered, about 15 pages, five tables (merges, a repeated header row, a shaded key column), three PNG figures, one with an SVG twin, first-page and default footers "Page n of N", a French "Page n sur N" section, a header picture, web, `mailto:` and internal links, degree signs, no-break spaces and en dashes, two comment threads and tracked changes by a second author including a deleted heading; the second version: a heading added, a paragraph moved, a row changed, a footnote inserted before another, a footer edited. Word updates the TOC before saving, so its copy holds Word's own TOC pages |
| `record` | authored, LibreOffice, Word, Word then Google | French style ids from Word, an unused linked pair, headings "1." "1.1." bound to a list, a TOC with `\u` next to a vertical merge, about 12 pages and 10 tables (a grey key column, a dated history table, wide matrices with merged headers, pale run shading), 20 content controls, comments by two people with a two-paragraph thread and reply, one resolved, about 30 tracked changes by one author, section margins away from Word's defaults |
| `text-deck` | authored, LibreOffice, Google, PowerPoint | 15 slides on two layouts, one hidden, notes on half, text boxes only in one font, `noAutofit` on every body, boxes that fit at 100 % of their width and overflow at 95 %, a rounded rectangle with text, percentage line spacing on bullet levels, pictures, connectors with arrowheads, a group, one slide with a transition |

If the families add more than 20 s to the Linux build job (render layer A
hashes every page outside `long-*`), the second version and the LibreOffice
copies move to the nightly render set in the same pull request.

## Links

- [A test suite from scratch](2026-11-20-test-suite-from-scratch.md),
  [Corpus at scale and behaviour tests](2026-11-21-corpus-at-scale-and-behaviour-tests.md),
  [Render fidelity](2026-11-21-render-fidelity.md) (layer B: Word's page
  counts), [Skill helpers move into the library](2026-11-20-skill-helpers-in-the-library.md),
  [Comparison: bookmark ids](2026-11-21-comparison-bookmark-identities.md),
  [Comparison: moved footnotes](2026-11-21-comparison-moved-footnotes.md)
- New issues #340 (TOC bookmarks between paragraphs), #341 (run shading),
  #342 (linked style pair), #343 (Python lengths); #328 (pptx render layer A)
