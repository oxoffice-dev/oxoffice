---
date: 2026-11-20
status: draft
scope: cli, python, docs
---

# Known limitations, readable from the CLI and Python

## Context

Agents need to know what the installed oxoffice cannot do yet, and what to do
instead, before they hit it. The skills used to keep that list in their own
`gaps.md` files, which drift from the build the agent actually has. The list
belongs next to the code, and the installed build should be able to say it.

## Decision

- One file, `limitations.toml` at the repository root, one entry per
  limitation:

  ```toml
  [[limitation]]
  id = "docx-compare-toc-bookmarks"
  format = "docx"            # docx, pptx, xlsx or all
  area = "compare"
  title = "Redline of two versions whose TOC bookmarks were renumbered"
  symptom = "compare refuses: duplicate bookmark id, or cannot revise paragraph boundary structures"
  workaround = "remove the _Toc bookmarks from both copies, compare, then rebuild the TOC on the result"
  issue = 12                 # the GitHub issue that tracks the fix
  ```

- It is embedded at build time, so the list always matches the installed
  version:
  - `oxoffice limitations [docx|pptx] [--json]`
  - `oxoffice.limitations(format=None)` returns a list of dicts
  - `oxoffice_cli::limitations()` for Rust callers
- `docs/LIMITATIONS.md` is generated from the same file, and CI checks that it
  is up to date.
- An entry is removed by the pull request that fixes it. Each entry has an
  issue, so the history lives there.
- The skills tell agents to run `oxoffice limitations docx` when a task
  touches a listed area, instead of keeping their own list.

## First entries

From the skills' current lists, still present in 0.1.0:

| Format | Limitation | Workaround |
| --- | --- | --- |
| docx | Redline refused when `_Toc` heading bookmarks were renumbered | remove the `_Toc` bookmarks, compare, rebuild the TOC |
| docx | Redline refused when a footnote reference moved | compare that pair in Word |
| docx | Python replacement slower than the CLI (about 1.5×) | `oxoffice docx replace --map` when only replacing |
| docx | Edits cost more as the body grows, most on files saved by Word or Google Docs | batch the edits; see the performance item of the roadmap |
| docx | Form operations cost seconds on long documents | pass every rule in one call |
| pptx | A part with a control character is refused at open | remove the character from that part |

## Plan

1. `limitations.toml`, the CLI command, the Python function, the generated
   page and its CI check.
2. One issue per entry, linked from the file.
3. The skills switch to `oxoffice limitations`.
