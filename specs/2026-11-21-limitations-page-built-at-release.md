---
date: 2026-11-21
status: accepted
scope: release
---

# The limitations page is built at release

## Context

`docs/LIMITATIONS.md` is generated from `crates/oxoffice-cli/limitations.json`
by `scripts/limitations_doc.py`, committed, and checked by CI to match the
JSON ([the limitations spec](2026-11-20-known-limitations-in-the-cli.md)).
Every pull request that adds or removes a limitation regenerates it, and
parallel pull requests then conflict on a file nobody edits by hand. The
page on `main` also describes `main`, not the release a user installed.
`oxoffice limitations` and `oxoffice.limitations()` already give each build
its own list.

## Decision

- `docs/LIMITATIONS.md` leaves the repository and `.gitignore` lists it.
- The release builds the page for its tag and attaches it as
  `oxoffice-X.Y.Z-limitations.md`, covered by `SHA256SUMS` and the
  provenance like every asset. Its link to the JSON points at the tag.
- `scripts/limitations_doc.py` prints the page (`--ref` for the link, `-o`
  to write a file, such as the ignored `docs/LIMITATIONS.md` for a local
  look). Lint still runs it, so an entry the page cannot render fails CI;
  the Rust tests of `oxoffice limitations` keep checking the entries.
- Lint also fails when `docs/LIMITATIONS.md` is tracked again: branches
  that edited the page get a modify/delete conflict once, resolved with
  `git rm docs/LIMITATIONS.md`, and `.gitignore` cannot stop a `git add`.
- The release step fails when the page it wrote is empty or missing.
- `oxoffice limitations` stays the source users are pointed to.

There is no documentation site yet; when there is one, it builds the page
the same way.

## Plan

One pull request: the script, the lint step, the release step and asset
table, `.gitignore`, `docs/DEVELOPMENT.md`, and rule 8 of `CLAUDE.md` (its
new wording approved by the maintainer on 2026-11-21). Checked by running
the script as lint and the release do, and by the next release dry run from
`main` after the merge (a dry run takes the script from `main`).

## Amendment 2026-11-21: one file per limitation

With the page gone, `crates/oxoffice-cli/limitations.json` itself stayed the
first source of conflicts: 14 open pull requests edited it, a fix that
removes an entry conflicted with one removing the entry next to it, and
each conflict cost a new CI run (#81 after #173 that morning).

- Each limitation is its own file, `crates/oxoffice-cli/limitations/<id>.json`,
  one JSON object whose `id` is the file name. Adding or removing one touches
  that file only, and two entries can no longer share an id.
- `build.rs` joins the files in file name order into the JSON array that
  `oxoffice_cli::LIMITATIONS_JSON` embeds, unchanged: the CLI, Python's
  `oxoffice.limitations()` and their tests read the same string as before.
  The build fails, naming the file, on a file that is not JSON, not
  `<id>.json`, whose `id` differs from its name, or whose name is not
  lower-case ASCII letters, digits and dashes (two names differing in case
  would be one file on Windows and macOS). Entries now list in id order.
- `scripts/limitations_doc.py` reads the directory in the same order.
- `scripts/limitations_move.py`, a one-shot tool, moves an open branch's
  edits of the old file into the directory during its rebase; it goes once
  no open branch needs it.
- Rule 8 of `CLAUDE.md` names the directory (wording approved by the
  maintainer on 2026-11-21).

A second pull request, merged right after the first, so that open branches
rebase once onto both.

## Links

- [Known limitations, readable from the CLI and Python](2026-11-20-known-limitations-in-the-cli.md)
