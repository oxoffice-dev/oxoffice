---
date: 2026-11-21
status: draft
scope: refactor
---

# Code inspection and refactoring

## Context

ROADMAP item 2: read the code with fresh eyes before building on it. The
inspection below was read-only, on `main` at 72cda9c.

- **Ten files hold half the code**: 252,000 of the workspace's 519,000 lines
  of Rust.

  | File | Lines |
  | --- | --- |
  | `oxoffice-docx/src/document.rs` | 55,554 |
  | `oxoffice-pptx/src/lib.rs` | 34,149 |
  | `oxoffice-docx-oxml/src/text.rs` | 31,260 |
  | `oxoffice-docx/src/field.rs` | 26,314 |
  | `oxoffice-docx/src/comparison.rs` | 24,608 |
  | `oxoffice-docx-py/src/document.rs` | 19,871 |
  | `oxoffice-docx-layout/src/engine.rs` | 17,342 |
  | `oxoffice-docx/src/bibliography.rs` | 16,266 |
  | `oxoffice-docx-oxml/src/document.rs` | 14,176 |
  | `oxoffice-chart/src/lib.rs` | 12,849 |

- **No unit test is left in `src/`** (0 `#[test]`, 0 `mod tests`), yet about
  2,500 lines sit under `#[cfg(test)]`: counters, traces, fault injection.
  Clippy compiles them through `--all-targets`; nothing runs them.
- **About 2,000 lines sit under `#[cfg(debug_assertions)]`**: work counters
  exposed to Python, and cross-checks that run the whole-document route next
  to each fast route.
- **The safety net for moving code is thin.** The Rust integration tests are
  a few hundred lines, no test renders a Word document, and the chart crate
  has no tests. The compiler is the real guard of a pure move.

## Decision

### File by file

A split follows the responsibilities that are already in the file. Where the
code is one dense job, size alone is not a reason.

| File | What it really holds | Verdict |
| --- | --- | --- |
| docx `document.rs` | `impl Document` (27,700 lines) in about 25 contiguous feature groups (lifecycle, staging, save, stories, notes, body, media, headers and footers, numbering, styles, sections, settings, fonts, properties, merge, TOC, replace, template hooks, render, inspect); the main-part partition and its proofs; story source editing and scanning; identifiers; tree walkers; option types; validation | **Split** into `document/`, about 45 modules. Feature groups share only a few hubs (`commit_staged_mutation`, `clone_for_staging`). The core machinery stays dense: the split makes it findable, not decoupled. |
| pptx `lib.rs` | About 20 responsibilities: state, raw XML owners, stored validation, open, save and commit, render and export (5,600 lines, all behind `render`), masters and themes, slide import, media, shape handles, arrange, headers and footers, properties, fonts, charts, comments, transitions | **Split** into about 20 modules, `render/` as one gated module. `Presentation` stays in `lib.rs`, as `html.rs`, `odp.rs` and `pdf.rs` already assume. |
| docx-oxml `text.rs` | Fields (46%: model, parse, write, rewrite), `CT_P`, `CT_R`, and the namespace and emission infrastructure the whole crate imports (`FieldXmlEmission`, `raw_with_external_bindings`, `WordOperationWork`) | **Split** into `text/`: bindings, owner check, work, emission, run, `field/`, `paragraph/`. |
| docx `field.rs` | About 10 unrelated features: TOC, mail merge (4,700), fragment import and identity remapping (3,900), index and tables of figures and authorities, legacy form fields, evaluator, formulas, span scanning, REF copies, fields in other stories | **Split** into `field/`, about 15 modules. |
| docx `comparison.rs` | One recursive engine (12,700 lines, body to paragraph, table or control to cell and back, one shared context), postconditions, source snapshots; plus comment snapshots and footnote custody, which accept and reject also use | **Trim only.** Delete 580 dead lines; move comments and footnotes to `comparison/`. Cutting the engine by element kind would spread one algorithm over files with about 300 visibility changes and import cycles. Revisit with items 3 and 6. |
| docx-py `document.rs` | 74 Python classes in families (values, sub-handles, styles, properties, forms, mail merge, content controls, exports), the owner inventory that keeps live handles valid (7,100 lines), and the single `#[pymethods] impl PyDocument` (5,155 lines) | **Split** by class family into `document/`. The `#[pymethods]` block moves whole: PyO3 allows one per class, and `multiple-pymethods` stays off. |
| docx-layout `engine.rs` | Stages with one-way dependencies: source registry, field formatting, SEQ, revision projection, caches, the layout transaction, paragraph layout, field substitution after pagination, structure, section geometry, headers and footers, fonts and East Asian text, bidi | **Split** into `engine/`, about 17 modules. The boundary with `paginator.rs` is sound. The two giant functions (1,700 and 1,800 lines) are not touched. |
| docx `bibliography.rs` | 48% data (APA languages 4,100, citation grammars 3,500); sources, update state, citations and rendering in between | **Split** into `bibliography/`, one PR. Low coupling, and item 5 will grow it. |
| docx-oxml `document.rs` | Two unrelated halves: the crate-wide admission and publication machinery (68%) and the document model (`CT_Body`, `CT_Document`, `CT_SectPr`) | **Split** into `document/section.rs` and `document/source.rs`; about 10 items cross the halves. |
| chart `lib.rs` | The ChartML model with parse and write (7,600, one dense job), the renderer (3,700), authoring on part bytes (1,600) | **Mostly harmless.** One cheap cut: `render.rs` and `authoring.rs`, about 6 visibility changes. The model stays whole. |

### How a split is done

- **A pure move.** Whole items (functions, types, constants, impl blocks, or
  a contiguous run of methods wrapped in a new `impl` block) move with their
  doc comments and attributes, unchanged.
- **`X.rs` stays the parent** next to an `X/` directory, as the story
  paragraphs work does with `document/story_paragraphs.rs` (#18). Types
  with private fields stay in the parent when their users end up in several
  children.
- **The only other edits:**
  - `use super::*;` at the top of each child;
  - `mod`, `use` and explicit re-export lines in the parent, so every path
    (`crate::x::Y`, public `oxoffice_*::x::Y`) still resolves;
  - visibility widened to `pub(super)` (or `pub(crate)` below a crate root)
    where the compiler asks;
  - the few struct literals built outside their new module.
- **No rename, no reordering inside an item**, and no new explicit import of
  a helper whose name exists elsewhere with another meaning (`local_name`,
  `attribute_in_namespace`, `namespace_declaration`,
  `raw_element_has_child_content`): next to a glob, a local name wins
  silently. Such helpers move with their callers.
- **Cut lists name items, not line numbers**, so a split is re-cut on a fresh
  `main` instead of rebased.
- **Checks on every split:**
  - clippy `--all-targets --all-features -D warnings`, plus the feature sets
    CI does not build for that crate (pptx: no default features,
    `default-template` alone, `render` alone, one release build);
  - the existing tests;
  - `git diff --color-moved=dimmed-zebra`;
  - `scripts/check_pure_move.py`, added with the first split: the lines
    removed and the lines added are the same multiset once visibility,
    `use` and `mod` lines and blank lines are set aside;
  - for Word layout code, which no test renders: the corpus rendered to PDF
    and PNG with the CLI before and after, bytes compared.

### Test-only code and debug instrumentation

Three kinds, three fates.

1. **`#[cfg(test)]` code: delete, about 2,500 lines.** Counters, `eprintln!`
   traces, fault injection (`FAIL_NEXT_*`, `FAIL_AFTER_COMPARISON_STAGING`),
   forced legacy routes, and diagnostics read from environment variables only
   under `cfg(test)` (`OXOFFICE_POLICY_DIAGNOSTICS`,
   `OXOFFICE_DESTINATION_REPAIR_DIAGNOSTICS`). Every item is only written.
   - Since clippy compiles them, a crate's definitions and their call sites
     go in the same pull request.
   - The two `cfg(debug_assertions)` `eprintln!` in `comparison.rs` that
     read the same variable go with them.
   - The seven field-summary checks in `text.rs` go too. They have not run
     since the tests left, and could come back as `debug_assert_eq!`.
   - `text.rs`'s `_publication` (in `completed_result_publication_summary`)
     runs in every build and can fail: production, untouched.
2. **Debug work counters: delete, about 750 lines.**
   - What goes:
     - the counters (`STAGING_WORK`, `REOPEN_WORK`, `TRANSACTION_*`,
       `COMMIT_WORK`, `SCOPE_GENERATION_WORK` and others) and their `take_*`
       functions;
     - the 13 debug-only functions of `oxoffice.docx._native`
       (`_staging_work`, `_reopen_work`, `_word_*`...), and pptx
       `_commit_work` and `_inventory_work`;
     - the opc `set_reread_zip_note` hook;
     - two switches nothing sets: `force_staged_properties` and
       `_word_force_checked_route`;
     - `DIRECT_FRAGMENT_BYTES`, which also counts in release builds.
   - Nothing reads their values. `conftest.py` only probes `_staging_work`
     to detect a debug build, so the bindings get an explicit `_debug_build`
     flag instead.
   - If item 3 wants work counts rather than wall time, it brings back the
     few it needs, named after what they measure.
3. **Debug cross-checks: keep, about 1,200 lines.** Each runs the
   whole-document route beside the fast route and asserts that they agree
   (`assert_same_reopen`, `assert_same_canonicalization`,
   `assert_same_commit`, `saves_as` against `to_bytes`,
   `assert_word_pieces_rebuild`...). They are real invariant checks, and the
   reason a debug build pays the whole document on every edit. Whether they
   move behind an explicit switch belongs to item 3, which owns the cost of
   edits.

The Word layout engine also carries cache hit and build counters that are
written in every build and never read; they go with its test code.

### Dead code that is not instrumentation

Hidden `pub` items that nothing calls, so the compiler cannot warn:
- six `ScopedWordOperation` methods in docx-oxml `document.rs`, about 350
  lines;
- `drop_w14_paragraph_identities` in `text.rs`. First check that the facade
  did not lose a call it needs: its doc says a copy must not share
  `w14:paraId`;
- `layout_paragraph` in `engine.rs`;
- the chart crate's dead dev-dependencies.

They go in one pull request, after a search of the whole repository,
Python, wasm and CLI included.

### Code shared between Word and PowerPoint

| What | Size | Fate |
| --- | --- | --- |
| Embedded content and signatures (`embedded.rs` in both facades) | 940 lines: 22 identical functions, 16 diverged | To `oxoffice-opc`. Not a pure move: Word checks that an internal target is a normalised pack URI and checks the content type of signature parts; PowerPoint does neither. The shared code keeps Word's checks, tested on PowerPoint files. |
| Python helpers: property classes, fonts, signing, slice indexing | 950 | To `oxoffice-py-support`. They diverge: `created`, `modified` and `last_printed` are a `datetime` in `oxoffice.docx` and a `str` in `oxoffice.pptx`. |
| CLI helpers and the clap font, sign and verify commands | 530 (9 identical) | To `oxoffice-cli-support`. They diverge: Word's `verify-signatures` verifies re-serialised bytes, PowerPoint the file as opened. |
| Font embedding model (`FontEmbeddingLicense`, `EmbeddedFontKind`, sfnt table directory) | 100, and it unblocks the font parts of the two rows above | To `oxoffice-layout`. |
| XML micro-helpers (`emit_raw` in 26 files, `push_attributes`, `capture_event`, NCName and XML 1.0 character checks, escaping) | 450 | To `oxoffice-core`, identical copies only. `local_name` copies that split at the last colon differ from core's on malformed names and stay until checked. |
| HTML import CSS and DOM helpers, ODF reader skeleton | 600 | Not now: the copies diverge in diagnostics and escaping, and sharing them needs a new crate. |
| `inverse_transform` (3 copies), roman numerals (4) | 110 | Not now: the copies differ on singular matrices and limits, so merging them is a behaviour decision. |

Kept apart on purpose: Word's `apply_tint_shade`, the `wp:` drawing code,
Word's `page_alphabetic`.

### Not in this item

- **Binding logic that belongs in the facade**: docx-py's owner inventory
  (7,100 lines) and `StyleFormatting`. Moving it means redesigning how an
  edit reports what it touched, which is item 3's ground, not a move.
- **Giant functions**: `layout_transaction` (1,700 lines),
  `layout_paragraph_with_source_and_table` (1,800),
  `qualify_constructor_failures` (1,100), `import_fragment_content_with_state`
  (1,100), `project_complex_fields` (900) and about 20 more over 200 lines.
  Breaking them up is not mechanical; it comes one function at a time, when
  work leads there.
- **Findings that may be bugs**, checked and filed as issues rather than
  fixed here:
  - PowerPoint's weaker embedded-content checks;
  - Word's `verify-signatures` verifying re-serialised bytes;
  - `drop_w14_paragraph_identities` never called;
  - Word's relationship-id reservation yielding `rId0` where opc starts at
    `rId1`;
  - PowerPoint layout edits missing from builds without `render`.

## Plan

One topic per pull request. Waves follow who is editing what, so that no
pull request rebases another session's work.

**Wave 1, now: files no other session is editing.**

1. This spec.
2. `chore(docx): drop test-only instrumentation from the word model`:
   docx-oxml `text.rs`, `document.rs`, `revision.rs`, `run_properties.rs`,
   `properties.rs`, `numbering.rs`, `borders.rs`, `math.rs`, `drawing.rs`,
   `ruby.rs`, `table.rs`, `content_control.rs`.
3. `chore(layout): drop test-only instrumentation from layout and opc`:
   docx-layout `engine.rs`, layout `font.rs`, opc `encryption.rs`.
4. `refactor(docx): split the word model document into document, section
   and source`: docx-oxml `document.rs`, `document/{section,source}.rs`,
   `scripts/check_pure_move.py`.
5. `refactor(docx): word model infrastructure and runs in their own
   modules`: `text/{bindings,owner_check,work,emission,run}.rs`.
6. `refactor(docx): word model fields in their own modules`: `text/field/`.
7. `refactor(docx): word model paragraphs in their own modules`:
   `text/paragraph/`.
8. `refactor(layout): split the word layout engine`, in two pull requests:
   the leaves first (field formatting, SEQ, revisions, fonts, bidi,
   geometry, drawings), then caches, headers and footers, substitution,
   structure, sources, the transaction and paragraph layout.
9. `refactor(chart): renderer and authoring in their own modules`, plus the
   dead dev-dependencies.

**Wave 2: oxoffice-docx files that the facade work in flight does not
touch.** None of these edits `document.rs` or `lib.rs`. They start on the
orchestrator's go.

10. `chore(docx): drop the dead comparison diagnostics`, if its test items
    do not reach into `document.rs` (otherwise it joins pull request 13).
11. `refactor(docx): comparison comments and footnotes in their own modules`.
12. `refactor(docx): split field.rs`, in three pull requests:
    - insert, legacy forms, formulas, generated tables and the bibliography
      side;
    - mail merge and fragment import;
    - scanning, TOC, REF copies, evaluator, update and story sources.
    Then `bibliography.rs`, in one pull request.

**Wave 3: after the facade, bindings and shape-walk work in flight merge,
on the orchestrator's go.**

13. `chore(docx): drop test-only instrumentation from the word facade and
    bindings`: docx `document.rs`, `content_control.rs`, `comments.rs`,
    `revision.rs`, `field.rs`, `redaction.rs`; docx-py `document.rs`,
    `paragraph.rs`, `table.rs`, `run.rs`.
14. `chore(pptx): drop test-only instrumentation`: pptx `lib.rs`, `html.rs`.
15. `chore: drop the debug work counters and their python probes`:
    - docx `document.rs`, `lib.rs`, `field.rs`;
    - docx-py `document.rs`, `lib.rs`, `table.rs`, `paragraph.rs`, `run.rs`;
    - pptx `lib.rs`, `embedded.rs`;
    - pptx-py `presentation.rs`, `lib.rs`, `shape.rs`;
    - py-support `lib.rs`;
    - opc `package.rs`, `lib.rs`;
    - `crates/oxoffice-py/tests/conftest.py`.
16. Split docx `document.rs`, in four pull requests:
    - the leaves (options, story types, numbering types, validation, text
      report, inspection);
    - two pull requests of `impl Document` feature groups;
    - the core machinery (identifiers, namespaces, main part, story editing
      and scanning, tree walkers, staging, save, story sources, element
      routes).
17. Split docx-py `document.rs`, in three pull requests: the class families,
    the owner inventory, then the internals and the `#[pymethods]` block.
18. Split pptx `lib.rs`: one pull request, one compiling commit per module
    (`render/`, `xml_owner/` and `package.rs`, the feature modules,
    `shapes/`).
19. `chore: remove hidden public items nothing calls`.

**Wave 4: shared code.** These are not moves: each changes code paths and
comes with tests.

20. XML micro-helpers to `oxoffice-core`.
21. The font embedding model to `oxoffice-layout`.
22. Embedded content and signatures to `oxoffice-opc`, PowerPoint gaining
    Word's checks.
23. Python helpers to `oxoffice-py-support`, once the `datetime` question is
    settled.
24. CLI helpers to `oxoffice-cli-support`, once the `verify-signatures`
    divergence is understood.

**Order with item 3.** Item 3 will change docx `document.rs` (staging, save,
publication), docx-oxml `text.rs` and `document.rs`, and the docx-py owner
inventory. The moves of those files (4 to 7, then 13, 15, 16, 17) go first,
then the performance code, so that neither rebases over the other. Item 3's
spec and measurements go on in parallel.

## Links

- [ROADMAP.md](../ROADMAP.md), items 2 and 3.
- [A test suite from scratch](2026-11-20-test-suite-from-scratch.md): why no
  unit tests are left.
- [docs/ARCHITECTURE.md](../docs/ARCHITECTURE.md): crates and the dependency
  rule.
