---
date: 2026-11-21
status: accepted
scope: tests
---

# Consumer workflows: the suite checks what a first user does

## Context

oxoffice has a first downstream user: tools that edit Word and PowerPoint
files through the Python package and the CLI, on rdocx 0.17.0 and rpptx 0.15.0
today, the code oxoffice was imported from (85c3b85). They will switch as
soon as oxoffice is ready, and the switch has to work the first time. Files
are edited in place, and people open the same files in Word, Google Docs and
Google Slides between two runs. The kinds of files the tests need:

- **a long versioned report**: a TOC field in a content control, several
  footer parts, figures, rendered to PDF and redlined between versions,
  often saved last by Google Docs;
- **records co-edited in Word and Google Docs**: localised style ids, many
  tables and content controls, comments and tracked changes by named people;
- **text-only decks** edited in Google Slides.

This spec numbers what those tools rely on: workflows W1 to W13, invariants
I1 to I12 and past breakages R1 to R14. Spreadsheets wait for the
spreadsheet facade (ROADMAP).

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
documents, but no test walks a workflow on a producer's file, no test pins a whole set of CLI JSON
keys, the deck text layout has no test at all, and
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
   session). Budget: **about 30 s more on the Linux build job of a pull
   request for the whole plan**, at most 2 minutes more on `main`. Job
   timings vary by minutes between runs of the same tree, so each pull
   request measures the tests it adds on their own (PR set and `full` set)
   and states both in its body.
3. **What breaks** becomes an issue and a strict known failure
   (`#[should_panic(expected = ...)]`, or `xfail(strict=True)` in Python),
   and a limitation entry when users can work around it. A walk over every
   producer's copy cannot carry one `should_panic` for one bad copy: its
   known failures per file live in a list checked both ways, like
   `known_failure` in the corpus manifest and `tests/render/known.toml`
   (`tests/corpus/workflows-known.toml`: file, workflow, issue), so a fix
   flips the test until the line goes. Open issues a
   workflow meets are listed below, so that their fixes come first within
   their severity (label `priority: workflow`).
4. **A migration note**, `docs/MIGRATING-FROM-RDOCX.md`, lists every name,
   flag, key, default and refusal that differs from rdocx 0.17.0 / rpptx
   0.15.0, with what to write instead. A Python test fails when a public name
   of rdocx 0.17.0 / rpptx 0.15.0 is neither in oxoffice nor in the note.
   No compatibility aliases.
5. **The minimum that covers the workflows.** A test is added where a
   workflow runs end to end on producer files, where a tool reads a CLI key,
   for the migration note, and for the bugs a workflow meets; coverage for
   its own sake is left out (synthetic matrices the corpus copies already
   hold, a budget table, Markdown, the SVG twin of a figure). **One corpus
   family**, `record`, saved by Word then Google Docs: no corpus file takes
   that path today, and it is the one co-edited files take. The existing
   copies carry the other walks: the contract (comments, replies, tracked
   changes by two authors, footers), the thesis (a TOC in a content control,
   numbered headings, footnotes, first-page footers), the training and pitch
   decks (rounded rectangles, connectors, a group, a hidden slide,
   transitions).
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
| I1 | Open and save without an edit keeps every part byte for byte; after an edit, untouched parts byte for byte, unmodelled XML kept | covered without an edit, partly after | `corpus::every_document_saves_every_part_unchanged` (every docx); edits keeping producer parts in `comments`, `properties_and_settings`, `fonts`, `paragraphs_and_styles` | Kept by the edit walk: every part it does not touch byte-equal (PR 4) |
| I2 | Same input, same calls, same build: same bytes, page count, layout and pixels | partly | corpus round trip; render baselines (hashes committed, so across processes; across platforms at night); `pdf::the_same_document_converts_to_the_same_bytes` | Not added: the render baselines and the round trip hold it |
| I3 | Every text read shows the accepted view; a replacement inside a tracked insertion stays tracked | partly | `revisions_and_compare::the_text_of_a_document_shows_insertions_and_hides_deletions`, `story_paragraphs::tracked_changes_read_in_the_accepted_view` | A replacement inside a tracked insertion, in the edit walk (PR 4) |
| I4 | Files Google Docs saved open, read and edit (decimal measurements, duplicate drawing ids, packed fields, duplicate and multiple default styles, `w:val="0"`, default namespaces) | partly | the 11 Google copies (decimal `tblW`, `w:col`, `w:spacing`; duplicate `docPr`; packed `PAGE` / `NUMPAGES`; default namespaces) through the round trip and the render | No synthetic matrix: the Google copies hold it; #340 (Google's TOC bookmarks) strict in PR 5 |
| I5 | Localised style ids are found and assigned by the id the file uses | covered | `fields_and_toc::rebuilding_the_toc_of_a_word_file_saved_in_french_finds_its_headings`, `paragraphs_and_styles::headings_are_found_in_the_word_and_google_reports_alike`, `comments::a_comment_added_to_a_localized_word_file_uses_its_comment_styles` | Kept by the walks on the French Word copies. Open: #169, #144 |
| I6 | Refuse rather than write something wrong; a batch is all or nothing; the input is unchanged on refusal | covered | `text_and_runs::counted_replacement_changes_nothing_when_a_count_is_wrong`, `checked_replacement`, `save::saving_over_the_opened_file_is_refused`, CLI `docx_replace_reach` | CLI `replace --map` with a wrong count writes nothing (PR 3). Different: fields the update cannot compute keep their cache with a diagnostic, a PDF that drops content comes with diagnostics |
| I7 | CLI JSON keys stay, or change in a release note | partly | `docx_stories` (`text --json`, `count --json`), `docx_comment_dates` (`comment list`, `comment add`), `pptx_duplicate_comments` (`pptx comment list`), `pptx_shapes` (`pptx shapes`) read the keys they need; `check_outputs.py` (`main`) a few more; no test pins a whole key set, none reads `diff`, `revision list` or `layout` | A keys-only snapshot of the commands the tools read, in those files (PR 3) |
| I8 | Page numbers from the layout are close to Word's and stable within a release | partly | page counts pinned implicitly by the render baselines | Word's page counts come with render layer B; the changelog rule for layout moves (PR 2). Open: #156 |
| I9 | Bytes in, bytes out, without touching the source | partly | `save::saving_elsewhere_leaves_the_source_as_it_was`, `test_save.py`, the corpus round trip | Covered enough; the edit walk saves elsewhere and reads the bytes back (PR 4) |
| I10 | A value a call accepted is still accepted, or the change is in the migration note | partly | unchanged since 0.17.0, untested | #343 strict in Python (PR 4) |
| I11 | No network, LibreOffice or Office at run time; bundled fonts | partly | the render tests use the bundled fonts; their set is pinned by the resources hash | Not added: the render tests hold it |
| I12 | No slowdown against rdocx 0.17.0 on these sizes | missing | scaling tests compare N with 16N only | Not added: the scaling tests stay the gate. Open: #5, #6, #234, #235 |

### Workflows

| | Workflow | Status | What covers it | Gap, and where it goes |
| --- | --- | --- | --- | --- |
| W1 | Read-only check of a document set: text of every story, runs and fonts, tables, comments, revisions, links, pictures, layout and page count, field instructions, validation, CLI `text`, `layout`, `revision list`, `validate` | partly | `story_paragraphs::every_story_of_the_corpus_reports_is_listed_with_its_kind_part_and_style`, `comments::every_producer_reports_the_corpus_comment_as_it_saved_it`, `revisions_and_compare::each_kind_of_tracked_change_is_read_with_its_author_and_date`, `inspection` (links, pictures, validation) | The read calls of the walks below, on producer copies; CLI `layout` and `revision list` keys (PR 3). Open: #146 |
| W2 | Diff a file against its last version, content and form | partly, different for form | no test of `docx diff`; comments and resolved state through the API | `docx diff --json` of the authored copy against each producer's: no text difference beyond a pinned list; one added comment shows as one row (PR 3). Different: `diff` compares text; form is compared from `text --json` runs |
| W3 | Counted edit in place, checked part by part before the write | partly | `text_and_runs` (replacements across runs, headers, footers, cells), `checked_replacement`, `paragraphs_and_styles::content_is_removed_by_index_or_by_half_open_range`, `tables::clone_table_row_copies_a_row_with_its_formatting_to_the_requested_place` | The edit walk on the contract copies: counted pairs across body, a cell, footers and a tracked insertion, a cloned paragraph, a removed range, then a guard (part set, untouched parts byte for byte, style, list level and runs of unchanged paragraphs) (PR 4). Open: #30, #172 |
| W4 | Page numbers from the layout: TOC to the field's depth, page fields, idempotent | partly, different for headers and footers | `fields_and_toc::rebuilding_a_word_toc_reuses_its_bookmarks_and_follows_its_switches`, `rebuilding_a_toc_follows_renamed_added_and_removed_headings`, `page_fields_take_their_numbers_from_the_layout_except_in_a_footer` | The thesis walk: rebuild and update twice, the second run changes no byte, entries equal the headings to the depth in `TOC n` styles, header and footer caches kept in every variant (PR 5); #340 strict. Open: #66, #156 |
| W5 | Render a version to PDF | partly | `render::every_page_of_a_corpus_render_has_a_page_size_of_its_document` (PDF pages equal layout pages), `pdf::the_same_document_converts_to_the_same_bytes` | PDF page count equals `page_count()` in the thesis walk (PR 5). Different: content the renderer drops gives a PDF and a diagnostic, not a refusal |
| W6 | Redline two versions, accept and reject round trip, PDF in legal colours | partly | `revisions_and_compare::word_and_character_granularity_mark_only_what_changed`, `a_redline_of_two_word_saved_reports_accepts_to_the_edited_copy`, `renamed_toc_heading_bookmarks_do_not_block_the_comparison`, `swapped_footnote_references_are_compared` | Two versions of a thesis copy (a heading added, a paragraph moved, a footnote inserted, TOC rebuilt) compared and round-tripped, legal colours probed (PR 5); #66 strict. Open: #68, #120 |
| W7 | Replace a figure keeping its size, SVG twin included | partly | `pictures::replacing_the_figure_of_a_report_keeps_its_size_and_caption`, `resizing_by_relationship_changes_every_picture_that_shows_it` | Not added: the picture tests cover it; the SVG twin of a figure has no corpus file and no test |
| W8 | Carry review comments from a Google-saved copy to a clean copy | partly | `comments::moving_a_comment_to_other_text_brings_its_replies`, `a_reply_is_linked_to_its_parent_and_covers_the_same_text` | Threads of the Google contract carried to the authored copy, read back with authors, dates, parents and resolved state (PR 4); on the Word-then-Google record (PR 7). Open: #182 |
| W9 | Comments and tracked changes in a named author's name, others' left alone | partly | `comment_on_text::tables_only_counts_the_matches_of_table_cells_only`, `revisions_and_compare::one_revision_is_accepted_or_rejected_by_its_id` | In the edit walk: comments and a redline by one author, accept and reject by id, the other authors' revisions and threads unchanged (PR 4) |
| W10 | Apply a house style to a record, idempotent | partly | `paragraphs_and_styles`, `lists::restart_numbering_gives_one_item_a_new_start_and_the_items_after_follow`, `tables::cell_shading_borders_margins_and_alignment_survive_a_save`, `test_table_properties.py` | #342 strict on the French Word report (PR 4); a restyle twice on the record (PR 7). Open: #82, #91, #234, #235 |
| W11 | A short document from scratch | covered | the behaviour modules | Opening in Word without repair stays on the Mac, on demand |
| W12 | Deck checks (text fit at 100 % and 95 % of a frame, shapes, ids, transitions) and touch-ups | partly; text layout missing | `shapes_walk`, `shapes_and_text::editing_text_keeps_the_formatting_of_the_run_it_replaces`, `replace_text_counts_every_match_across_slides_groups_and_tables`, `pictures::replacing_the_image_keeps_the_picture_geometry_name_and_alt_text`; renders in #328 | The deck walk on the training and pitch copies: `text_layout` of every frame at 100 % and 95 % pinned, a rounded rectangle's inset, transitions, arrowheads (PR 6) |
| W13 | Text for agents: JSON with run formatting, Markdown | partly | `docx_stories`, `test_story_paragraphs.py`, `html_and_markdown` | Keys of `text --json` (PR 3) |

### Past breakages

| | What broke | Status on `main` | Test, and where it goes |
| --- | --- | --- | --- |
| R1 | Google-saved files refused for decimal measurements and duplicate drawing ids | holds on the corpus; duplicate ids different (kept) | Not added: the Google copies hold it |
| R2 | A rebuilt TOC one page earlier than Word's | not measurable: no copy holds Word's own TOC pages | Word's own TOC pages: render layer B |
| R3 | TOC with `\u` refused next to a vertically merged cell | holds (synthetic probe) | Strict test in the thesis walk's module (PR 5); on the record (PR 7) |
| R4 | TOC refused over a heading deleted with tracked changes | holds (synthetic probe) | Strict test in the same module (PR 5) |
| R5 | A bare `0` length refused after an upgrade | holds, except `set_default_paragraph_format` | #343 strict (PR 4) |
| R6 | An unused linked style pair cannot be removed | breaks | Strict known failure on `word/report.docx` (PR 4); #342 |
| R7 | Removing a paragraph orphaned its comment; anchor text, moving a comment, paragraph-scoped replace | holds; paragraph-scoped replace untested | In the edit walk: no orphan marker in the saved XML (PR 4) |
| R8 | Comparison refused renumbered `_Toc` bookmarks and moved footnote references | holds with Word-style fields; breaks on oxoffice's own rebuilt TOC (`PAGEREF` as simple fields) | Strict known failure on #66, in the redline walk (PR 5) |
| R9 | Threads and resolved state keyed on a comment's first paragraph; a newline in a comment | holds; tests would not see a regression (reads accept any paragraph id) | XML pins on the last paragraph id in the edit walk (PR 4); Google's form on the record (PR 7) |
| R10 | Run shading not drawn | breaks | Pixel probe, strict known failure (PR 5); #341 |
| R11 | Replacement through Python 2 to 4 times the CLI | open | Not added; #5 |
| R12 | A release with a broken TOC; `NUMPAGES` caches | holds | The thesis walk on every copy on `main` (PR 5) |
| R13 | A text layout change in decks moved every height silently | no pin | The deck layout table (PR 6) and the changelog rule (PR 2) |
| R14 | Slide transition not readable | holds, untested | Read in the deck walk (PR 6) |

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
  macOS arm64 and x86_64, Windows, and `SHA256SUMS`.

### Open issues a workflow meets

Their fixes come first within their severity (label `priority: workflow`):
#66, #156, #146, #30, #172, #5, #6, #82, #91,
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
| 2 | `docs: the migration note from rdocx`: `docs/MIGRATING-FROM-RDOCX.md`, its rows kept in `tests/migration/differences.toml` (a pull request that edits only Markdown runs no test); the public names of rdocx 0.17.0 and rpptx 0.15.0 as a committed list (classes, members, keyword arguments, CLI commands and flags); a check that each is in oxoffice or in a row, and that each row names something real; the changelog rule for layout moves in `docs/DEVELOPMENT.md` | `crates/oxoffice-py/tests/test_migration.py`, `tests/migration/` | PR | 2 s |
| 3 | `test(cli)`: keys-only snapshots of what the tools read (`docx text`, `diff`, `revision list`, `layout`, `comment list`, `pptx text`, `fit`, `shapes`); `docx diff` of the authored copy against each producer's, differences pinned, one added comment = one row; `replace --map` with a wrong count writes nothing; `OXOFFICE_WORKFLOWS` in `ci.yml` (CI session) | existing files of `crates/oxoffice-cli/tests/` | PR (report, contract), `main` (every family) | 5 s |
| 4 | `test(docx)`: the edit and review walk (W3, W8, W9, I1, I3, R7, R9) on the contract copies; #342 and #343 strict | `checked_replacement.rs`, `comments.rs`, `paragraphs_and_styles.rs`, a part-comparison helper in `support.rs`; `crates/oxoffice-py/tests/test_lengths.py` | PR (Word copy), `main` (every copy) | 8 s |
| 5 | `test(docx)`: the page numbers, PDF and redline walk (W4, W5, W6, R3, R4, R8, R12) on the thesis copies; #340, #66 and #341 strict | `fields_and_toc.rs`, `revisions_and_compare.rs`, `pdf.rs` | PR (Word copy), `main` (every copy) | 8 s |
| 6 | `test(pptx)`: the deck walk (W12, R13, R14) on the training and pitch copies, gated on the `render` feature (the WebAssembly job builds `--test pptx` without it) | `shapes_and_text.rs`, `slides_and_layouts.rs` | PR | 3 s |
| 7 | `test(corpus)`: the `record` family (French style ids from Word, an unused linked pair, a TOC with `\u` next to a vertical merge, tables with pale run shading, content controls, comments by two people with a two-paragraph thread, tracked changes by one author), authored and saved by LibreOffice, then by Word and Word then Google on the Mac (a `word-google/` route in `produce.py`); the walks of PR 4 and PR 5 and a restyle twice on its copies | `tests/corpus/`, the modules above | PR | 6 s |

Order: 2 and 3 first (the switch needs them), then 4, 5 and 6 in any order,
7 last, with the Mac session. Pull requests 4 to 6 touch modules OX-D2 and
OX-L also edit: a line to each before each. A bug a walk finds follows
CLAUDE.md rule 10: one root cause, one issue, with its severity, area and a
neutral "Seen with".

## Links

- [A test suite from scratch](2026-11-20-test-suite-from-scratch.md),
  [Corpus at scale and behaviour tests](2026-11-21-corpus-at-scale-and-behaviour-tests.md),
  [Render fidelity](2026-11-21-render-fidelity.md) (layer B: Word's page
  counts), [Skill helpers move into the library](2026-11-20-skill-helpers-in-the-library.md),
  [Comparison: bookmark ids](2026-11-21-comparison-bookmark-identities.md),
  [Comparison: moved footnotes](2026-11-21-comparison-moved-footnotes.md)
- New issues #340 (TOC bookmarks between paragraphs), #341 (run shading),
  #342 (linked style pair), #343 (Python lengths); #328 (pptx render layer A)
