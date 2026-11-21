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
- `oxoffice limitations` stays the source users are pointed to.

There is no documentation site yet; when there is one, it builds the page
the same way.

## Plan

One pull request: the script, the lint step, the release step and asset
table, `.gitignore`, `docs/DEVELOPMENT.md`. Rule 8 of `CLAUDE.md` names the
file too and changes with the maintainer's word. Checked by running the
script as lint and the release do, and by the next release dry run.

## Links

- [Known limitations, readable from the CLI and Python](2026-11-20-known-limitations-in-the-cli.md)
