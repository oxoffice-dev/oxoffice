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
| 2026-11-21 | [Lighter CI on pull requests](2026-11-21-lighter-ci-on-pull-requests.md) | shipped |
| 2026-11-21 | [Nightly scaling on macOS and Windows](2026-11-21-nightly-scaling-on-macos-and-windows.md) | shipped |
| 2026-11-21 | [main builds on Linux, with one run waiting](2026-11-21-main-on-linux-with-one-waiting-run.md) | shipped |
| 2026-11-21 | [A faster build job](2026-11-21-faster-build-job.md) | shipped |
| 2026-11-21 | [Code inspection and refactoring](2026-11-21-code-inspection-and-refactoring.md) | accepted |
| 2026-11-21 | [Edit cost at the root](2026-11-21-edit-cost-at-the-root.md) | accepted |
| 2026-11-21 | [Fillable PDF forms](2026-11-21-fillable-pdf-forms.md) | accepted |
| 2026-11-21 | [Text list style levels live on the heap](2026-11-21-text-list-style-levels-on-the-heap.md) | shipped |
| 2026-11-21 | [Comparison: bookmark ids and generated TOC names are no change](2026-11-21-comparison-bookmark-identities.md) | shipped |
| 2026-11-21 | [Comparison: a footnote whose reference moved](2026-11-21-comparison-moved-footnotes.md) | shipped |
| 2026-11-21 | [Release wheels run on the main thread before publishing](2026-11-21-release-wheel-smoke-test.md) | shipped |
