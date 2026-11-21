---
date: 2026-11-21
status: accepted
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
- **The reviewer posts its findings on the pull request itself**, in one
  comment headed `🔍 Independent review` with the head it read, major and
  minor alike, each with `file:line`, the defect and a concrete failure
  scenario. A review with nothing to report says so. The pull request keeps
  the trace.
- **The author fixes the findings**, or answers in the thread why one is not
  a defect. A major finding is fixed unless the maintainer waives it in the
  thread. A substantial fix gets a new review of what changed.
- **Then CI green, squash merge.**
- Documentation-only and pure code moves get a light review, still posted:
  for a move, that nothing but location changed.

What a review looks for, most important first: wrong behaviour and missed
cases, tests that would pass with the feature broken, Rust, CLI and Python
disagreeing on the same feature, unsafe CI changes (unpinned actions,
secrets, steps that pass silently). Style only where it breaks a documented
convention.

## Plan

1. This spec and the review step in [docs/DEVELOPMENT.md](../docs/DEVELOPMENT.md).
2. Applied from now on to every open pull request.

## Links

- [docs/DEVELOPMENT.md](../docs/DEVELOPMENT.md), the loop a change follows.
