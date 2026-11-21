---
date: 2026-11-21
status: shipped
scope: process
---

# An independent review before every merge

## Context

Most pull requests are written by agents, often several in parallel. Until
now the only check before a merge was CI and a brief read by someone who
already knew what the author meant to do. That catches what the tests catch,
not a missed case, a test that asserts nothing, or a Python call that drifts
from its Rust twin.

## Decision

- **Every pull request gets an independent review before it is merged.**
  Independent means a reviewer who did not write the code: a fresh agent
  given only the pull request, its branch and the spec it links, never the
  author's reasoning, or a person.
- **Whoever works on the pull request or wants to merge it** runs that review
  or asks someone for it. No CI job: the review belongs to the merge, not to
  every push.
- **The review is posted on the pull request, unedited**, in one comment
  headed `🔍 Independent review` with the head it read. The reviewer posts
  it; an agent reviewer that cannot write to GitHub hands its report to
  whoever launched it, who posts it as is. Each finding has `file:line`, the
  defect and a concrete failure scenario. A review with nothing to report
  says so. The pull request keeps the trace.
- **Major or minor.** A finding is major when it would ship a defect: wrong
  behaviour or a missed case a user can hit, a test that would pass with the
  feature broken, Rust, CLI and Python disagreeing on the same feature, an
  unsafe CI change (unpinned action, exposed secret, a step that passes
  silently), a document that misstates how things work. Everything else is
  minor. The reviewer classifies, the author may contest the class in the
  thread, the maintainer decides.
- **The author answers every finding in the thread**: fixed, with the commit,
  or why it is not a defect. A major finding is fixed unless the maintainer
  waives it in the thread.
- **The review covers the head that is merged.** After the reviewed head,
  what cannot change behaviour needs nothing more: a rebase without conflict,
  a typo in prose. Any change to code, tests, workflows or the meaning of a
  document, fixes for findings included, gets a review of the delta, posted
  the same way.
- **Then CI green, squash merge.**
- **Light review** for documentation-only changes and pure code moves, still
  posted: for documentation, that links and anchors resolve and the text
  agrees with the code and the other documents; for a move, that nothing but
  location changed.
- Dependabot pull requests are reviewed too; the maintainer, or the agent
  that merges them, answers the findings.

What a review looks for, most important first: wrong behaviour and missed
cases, tests that would pass with the feature broken, Rust, CLI and Python
disagreeing, unsafe CI changes. Style only where it breaks a documented
convention.

## Plan

1. This spec, the review step in [docs/DEVELOPMENT.md](../docs/DEVELOPMENT.md)
   and a rule in [CLAUDE.md](../CLAUDE.md). Checked: from this pull request
   on, every merged pull request carries a `🔍 Independent review` comment
   that covers its merged head.

## Outcome

- Shipped in #23, itself reviewed that way: a first review (one major, four
  minor), then two delta reviews (minor findings only), each answered.
- An agent reviewer is sometimes refused the right to comment on GitHub by
  the permission system; whoever launched it then posts its report as is.

## Links

- [docs/DEVELOPMENT.md](../docs/DEVELOPMENT.md), the loop a change follows.
- #23, this change.
