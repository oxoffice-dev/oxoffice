# Specs

Timestamped, scoped records of what we decide to build and why.

`docs/` describes the project as it is today. `specs/` is the evolution log:
the plan before a change, then what shipped and what we learned.

## When to write one

Write a spec **before** any non-trivial change: a new feature, a public API
change in Rust, the CLI or Python, a format or output change, a refactor that
moves responsibilities, a test or release process change. The pull request
that implements it links the spec.

Skip it for bug fixes with an obvious cause, small internal refactors,
dependency bumps and documentation.

## Convention

- One file per topic, named `YYYY-MM-DD-kebab-name.md`. The date is the day
  the spec is opened. It never changes afterwards.
- Front matter:

  ```yaml
  ---
  date: YYYY-MM-DD
  status: draft | accepted | shipped | superseded | abandoned | reference
  scope: short tag (docx, pptx, xlsx, cli, python, perf, tests, release, ...)
  ---
  ```

- Sections:
  - **Context**: the problem, with numbers when there are any.
  - **Decision**: what we do, and what we deliberately do not do.
  - **Plan**: the steps or pull requests, and how each one is checked.
  - **Learnings** (or **Outcome**): added when the work ships.
  - **Links**: issues, pull requests, earlier specs.
- Keep it tight. A spec is a record, not a tutorial, and not a ledger.
- `draft` while it is discussed, `accepted` when work starts, `shipped` when
  it is merged. A spec that is replaced says `superseded` and links forward.
  Never delete one.

## Index

| Date | Title | Status |
| --- | --- | --- |
| 2026-11-20 | [A friendly fork of rdocx](2026-11-20-friendly-fork-of-rdocx.md) | accepted |
| 2026-11-20 | [Names, packaging and versions](2026-11-20-names-packaging-versions.md) | shipped |
| 2026-11-20 | [CI and release from scratch](2026-11-20-ci-and-release.md) | accepted |
| 2026-11-20 | [A test suite from scratch](2026-11-20-test-suite-from-scratch.md) | accepted |
| 2026-11-20 | [Skill helpers move into the library](2026-11-20-skill-helpers-in-the-library.md) | shipped |
| 2026-11-20 | [Known limitations, readable from the CLI and Python](2026-11-20-known-limitations-in-the-cli.md) | shipped |
| 2026-11-21 | [Corpus at scale and behaviour tests by area](2026-11-21-corpus-at-scale-and-behaviour-tests.md) | shipped |
| 2026-11-21 | [An independent review before every merge](2026-11-21-independent-review-before-merge.md) | shipped |
| 2026-11-21 | [Lighter CI on pull requests](2026-11-21-lighter-ci-on-pull-requests.md) | superseded |
| 2026-11-21 | [Nightly scaling on macOS and Windows](2026-11-21-nightly-scaling-on-macos-and-windows.md) | superseded |
| 2026-11-21 | [main builds on Linux, with one run waiting](2026-11-21-main-on-linux-with-one-waiting-run.md) | superseded |
| 2026-11-21 | [A faster build job](2026-11-21-faster-build-job.md) | shipped |
| 2026-11-21 | [A check without debug assertions on every pull request](2026-11-21-check-without-debug-assertions.md) | shipped |
| 2026-11-21 | [Code inspection and refactoring](2026-11-21-code-inspection-and-refactoring.md) | accepted |
| 2026-11-21 | [Edit cost at the root](2026-11-21-edit-cost-at-the-root.md) | accepted |
| 2026-11-21 | [Edit cost: publishing the one block an edit touched](2026-11-21-edit-cost-one-block-publication.md) | accepted |
| 2026-11-21 | [Fillable PDF forms](2026-11-21-fillable-pdf-forms.md) | accepted |
| 2026-11-21 | [Text list style levels live on the heap](2026-11-21-text-list-style-levels-on-the-heap.md) | shipped |
| 2026-11-21 | [Comparison: bookmark ids and generated TOC names are no change](2026-11-21-comparison-bookmark-identities.md) | shipped |
| 2026-11-21 | [Comparison: a footnote whose reference moved](2026-11-21-comparison-moved-footnotes.md) | shipped |
| 2026-11-21 | [A whole paragraph's simple field is tracked as Word tracks it](2026-11-21-simple-field-retired-with-its-paragraph.md) | shipped |
| 2026-11-21 | [Release wheels run on the main thread before publishing](2026-11-21-release-wheel-smoke-test.md) | shipped |
| 2026-11-21 | [Render fidelity: a baseline on every pull request, Word, PowerPoint and LibreOffice every night](2026-11-21-render-fidelity.md) | accepted |
| 2026-11-21 | [The limitations page is built at release](2026-11-21-limitations-page-built-at-release.md) | accepted |
| 2026-11-21 | [Python core property dates and shared helpers](2026-11-21-python-core-property-dates-and-shared-helpers.md) | accepted |
| 2026-11-21 | [Embedded content and signatures in oxoffice-opc](2026-11-21-embedded-content-and-signatures-in-opc.md) | accepted |
| 2026-11-21 | [Consumer workflows: the suite checks what a first user does](2026-11-21-consumer-workflows.md) | accepted |
| 2026-11-21 | [One rule for every Python length argument](2026-11-21-one-rule-for-python-length-arguments.md) | accepted |
| 2026-11-21 | [The release pushes main and the tag after every build is green](2026-11-21-release-pushes-after-the-builds.md) | shipped |
| 2026-11-22 | [One CI for pull requests and main, the full sets at night](2026-11-22-one-ci-for-pull-requests-and-main.md) | accepted |
| 2026-11-22 | [A short release: build, check each package, publish](2026-11-22-a-short-release.md) | accepted |
| 2026-11-22 | [Every release opens with its highlights](2026-11-22-release-highlights.md) | accepted |
| 2026-11-22 | [A smaller test suite: the docx tests that run 99 % of the lines](2026-11-22-a-smaller-test-suite.md) | accepted |
| 2026-11-22 | [PowerPoint binding: owners that do not depend on their slide position](2026-11-22-pptx-binding-owners-independent-of-slide-position.md) | shipped |
| 2026-11-22 | [A field operand that joins text and nested fields](2026-11-22-joined-field-operands.md) | accepted |
| 2026-11-22 | [A line break keeps where its text restarts](2026-11-22-line-break-clear.md) | shipped |
| 2026-11-22 | [Cut and pasted content keeps its bookmarks whole and its ids unique](2026-11-22-cut-content-keeps-its-bookmarks-and-ids.md) | accepted |
| 2026-11-22 | [Characters XML cannot carry: refused or dropped at the setter](2026-11-22-characters-xml-cannot-carry.md) | shipped |
| 2026-11-23 | [A deck draws the characters its fonts lack with a system font](2026-11-23-deck-system-font-fallback.md) | accepted |
| 2026-11-23 | [Date controls with a time, and the US Gregorian calendar](2026-11-23-date-controls-with-times.md) | accepted |
| 2026-11-23 | [One font record addressed by its index](2026-11-23-font-records-by-index.md) | accepted |
