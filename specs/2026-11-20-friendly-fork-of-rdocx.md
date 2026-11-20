---
date: 2026-11-20
status: accepted
scope: project
---

# A friendly fork of rdocx

## Context

[rdocx](https://github.com/tensorbee/rdocx) by Atul Sharma is a pure Rust
stack for Word and PowerPoint files: a typed OOXML model, a layout engine, a
PDF renderer, a CLI and Python bindings. It is a remarkable piece of work.

We use it every day through agents that edit documents saved by Word and
Google Docs. Our needs and timelines differ from upstream's: we want to ship
fixes as soon as we find them, make edits fast on real-world documents, and
build the test suite around the files our users actually have. A fork lets us
do that at our own pace, without loading the upstream queue with our
priorities.

## Decision

- Start oxoffice from the code that rdocx-skills v2026.11.16 deploys:
  tensorbee/rdocx v0.17.0 (`eb980e94`) plus our pull requests #448 to #454
  (`2543cf63` on hadim/rdocx). Fresh git history, with both SHAs in the first
  commit and in [NOTICE](../NOTICE).
- Keep the MIT licence and tensorbee's copyright notice, and every font and
  asset licence.
- Use a lighter process of our own: short specs, conventional commits and a
  plain CI (see [specs/README.md](README.md)). The upstream process files
  (`.claude/`, `.agents/`, `docs/`, `scripts/`) are not carried over.
- Start the test suite from scratch, planned from first principles in
  [2026-11-20-test-suite-from-scratch.md](2026-11-20-test-suite-from-scratch.md).
  The upstream tests stay available in its history when a case is worth
  mining.
- Upstream stays a source we can port from. We keep reporting bugs that also
  affect rdocx, with reproduction scripts.
- What we plan next is in [ROADMAP.md](../ROADMAP.md).

## Plan

1. Import and clean the tree (done in the first commit).
2. Rename to oxoffice and unify the packaging, see
   [2026-11-20-names-packaging-versions.md](2026-11-20-names-packaging-versions.md).
3. CI and release, see [2026-11-20-ci-and-release.md](2026-11-20-ci-and-release.md).
4. `CLAUDE.md`, `docs/`, `specs/`, `ROADMAP.md`.
5. The new test suite and its corpus.
6. v0.1.0.
7. `oxoffice-skills`, a plain skills repository on top of the releases.

## Outcome

To be completed when the bootstrap is done.
