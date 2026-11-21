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
  the same way. (Amended on 2026-11-21, see [A review budget](#amendment-2026-11-21-a-review-budget):
  fixes for minors get a proof instead of a delta review.)
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
   that covers its merged head (or the head the review budget allows, see
   the [amendment](#amendment-2026-11-21-a-review-budget)).

## Outcome

- Shipped in #23, itself reviewed that way: a first review (one major, four
  minor), then two delta reviews (minor findings only), each answered.
- An agent reviewer is sometimes refused the right to comment on GitHub by
  the permission system; whoever launched it then posts its report as is.

## Amendment 2026-11-21: a review budget

The night of 2026-11-20 to 21 gave 231 reviews for 79 pull requests: 2.9
each, up to 11. #81 had 7 and no major among them. A first review found a
major in 52 % of cases, a third or later review in 26 %: the waste was
fixes for minors triggering delta reviews.

The maintainer decided on 2026-11-21 (about 07:55 UTC):

- **At most two reviews per pull request**: one full review, then at most
  one delta review, after a major was fixed or after a change no finding
  asked for that can change behaviour (new code, a conflict resolved by hand
  in code, tests or workflows). A third review needs the maintainer's
  agreement; beyond the budget the maintainer decides from the evidence
  (mutation checks, `git range-diff`), without another round.
- **Minors never trigger a review**: fixed with a proof stated in the
  answer (a test that fails without the fix, or a mutation check), or a
  follow-up issue, or why it is not a defect. A minor that would need a
  risky change goes to a follow-up issue, not into the pull request.
  (Changed by the amendment below: a cheap minor is fixed in the pull
  request, one follow-up issue at most.)
- **A rebase or merge that only resolves a conflict in data**
  (`limitations.json`, a generated page, the `GROWING` or test-module lists,
  a moved context line) needs no review: `git range-diff`, the tests, and a
  line in the thread.
- **A delta review judges the delta only**: defects the new commits bring,
  or majors they fail to fix. No new finding on code the delta does not
  touch.
- A light review counts as the full review.

This replaces the delta review of every change after the reviewed head:
fixes for minors get a proof, not a review; a major fix, or a change no
finding asked for that can change behaviour, still gets its delta review.
[docs/DEVELOPMENT.md](../docs/DEVELOPMENT.md#independent-review) and rule 9
of [CLAUDE.md](../CLAUDE.md) say so.

## Amendment 2026-11-21: minors fixed in the pull request

The reviews of the day opened follow-up issues for minors, several per pull
request: 135 issues were opened on 2026-11-21, many of them review
follow-ups, while the goal is to close every issue before 0.2.0. The maintainer decided: a minor found in review is
fixed in the pull request when the fix is cheap, with its proof (a test that
fails without it, or a mutation check). A follow-up issue is opened only for
a real defect that the pull request cannot carry, and at most one per pull
request, grouping its findings. This replaces the minors bullet of the
budget amendment above; the budget of reviews is unchanged. Rule 9
of [CLAUDE.md](../CLAUDE.md) and
[docs/DEVELOPMENT.md](../docs/DEVELOPMENT.md#independent-review) say so.

## Links

- [docs/DEVELOPMENT.md](../docs/DEVELOPMENT.md), the loop a change follows.
- #23, this change.
