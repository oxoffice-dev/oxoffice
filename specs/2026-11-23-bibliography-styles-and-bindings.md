---
date: 2026-11-23
status: shipped
scope: docx, cli, python
---

# Bibliography: every source type in every style, from the CLI and Python

## Context

Roadmap item 5 ([#468](https://github.com/oxoffice-dev/oxoffice/issues/468)).
The Rust facade already reads and edits the source collection
(`bibliography_sources`, `add_` / `replace_` / `remove_bibliography_source`),
sets the style (`set_bibliography_options`), inserts `CITATION` and
`BIBLIOGRAPHY` fields (`insert_citation`, `insert_bibliography`) and rebuilds
their results (`update_bibliography`). What it formats is a catalogue of
branches recorded from Word, and any field outside the catalogue fails the
whole update with "catalogued ... still being implemented" (29 such
branches).

Measured on main (8b73691f): one source of each of the 17 kinds, cited once,
a bibliography inserted, `update_bibliography_with_default_locale`:

| Style | en-US (1033) | fr-FR (1036) | de-DE, ja-JP | en-US, `\p 12` |
| --- | ---: | ---: | ---: | ---: |
| APA 6th | 17/17 | 15/17 | 0/17 | 17/17 |
| the 11 others | 1/17 (Book) | 0/17 | 0/17 | 0/17 (IEEE and ISO 690 numeric: 1/17) |

There are no tests on the bibliography in the repository. The CLI and Python
have none of it: the README row says "from Rust only".

## Decision

### Scope

- **Every source kind in every style, in en-US**: the 17 kinds of
  `BibliographySourceKind` in the 12 styles of `BibliographyStyle`, both the
  bibliography entry and the citation. This includes the citation switches
  `\p \v \f \s \n \y \t`, several sources in one citation (`\m`), one to
  three, many and corporate authors, and missing fields as Word renders
  them. APA keeps the locales it has.
- **Word is the oracle.** A generated probe document for each style holds
  every kind with those variants. Word (the Mac session) updates its fields
  and saves it, and the saved files are committed as small fixtures. A test
  rebuilds the results with oxoffice and compares them with Word's cached
  results: text, then the run formatting we write (italic, bold, underline,
  quotes, the numbered or tabbed layout). No Word style sheet (XSL) is read
  or committed.
- **An uncovered field still fails the update**, with no partial
  publication. The error names what is missing:
  `bibliography: <style> has no <kind> <entry|citation> for locale <lcid> yet`.
  It replaces the catalogue's internal wording. Keeping a stale cache without
  saying so would be worse than refusing.
- **The CLI and Python reach the whole facade** (below), and the README row
  says what is covered.
- **Not in this work**: locales other than en-US for the 11 non-APA styles,
  the APA locales still missing (de-DE, ja-JP...), author disambiguation
  (two sources whose authors share a last name), complex-script runs. They
  go into one follow-up issue, opened when the bindings land, and into one
  limitation file (`docx-bibliography-locales.json`). Word styles that are
  not among our 12 (if the Mac's Word ships newer editions, for example APA
  7th) are added to this spec once the Mac session has listed them. `oxoffice
  docx fields update` keeps citation and bibliography caches as it does
  today: the bibliography update stays its own call.

### Rust additions

- Names for the CLI and Python, lossless and stable:
  `BibliographyStyle::{ALL, name, from_name}` and the same for
  `BibliographySourceKind`, `BibliographyContributorRole` and
  `BibliographySourceField`, with `FromStr` and `Display`. The names are
  snake case: `apa6`, `chicago`, `gb7714`, `gost_name`, `gost_title`,
  `harvard_anglia`, `ieee`, `iso690_author_date`, `iso690_numeric`, `mla7`,
  `sist02`, `turabian`; `journal_article`, `book_author`, `url`,
  `reference_order`, `patent_type`... (the XML name in snake case, except
  `RefOrder` -> `reference_order` and `Type` -> `patent_type`, as the enum
  says).
- `add_bibliography_source` returns the GUID it wrote (`Result<String>`).
  An empty `guid` asks for one: the SHA-256 of the authored source XML in the
  `{xxxxxxxx-xxxx-4xxx-8xxx-xxxxxxxxxxxx}` shape, taking the next value while
  the collection already holds it. The value is deterministic for the same
  source, and two different sources do not share it (Word's master list
  matches sources by GUID).
- `insert_citation_after_text(text, occurrence, &CitationOptions)`: the
  citation goes right after the `occurrence`-th match of `text` in the main
  story. It uses the same literal finder and checked run split as
  `add_comment_on_text` (case-sensitive, no match across paragraphs; a
  missing occurrence is an error and changes nothing).

### CLI: `oxoffice docx bibliography`

Style, kind, role and field values are the names above with `-` for `_`
(`--style harvard-anglia`). Every edit writes `-o OUT` and takes `--json`
for the operation record.

| Command | Does |
| --- | --- |
| `sources FILE [--json]` | lists the sources: tag, GUID, kind, locale, contributors, fields, diagnostics |
| `add-source FILE --source SRC.json -o OUT` | adds one source, or each source of a JSON array; prints the GUIDs |
| `replace-source FILE --source SRC.json -o OUT` | replaces sources by tag, keeping the GUID |
| `remove-source FILE TAG -o OUT` | removes an uncited source |
| `style FILE [--json]` | prints the saved style |
| `set-style FILE --style S [--locale N] [--filter-locale N] [--tag T]... -o OUT` | sets the style and the selectors of the existing bibliographies |
| `cite FILE (--after TEXT [--occurrence K] \| --paragraph N --run R) --source SPEC... [--locale N] -o OUT` | inserts a citation |
| `insert FILE --index N --style S [--locale N] [--filter-locale N] [--tag T]... -o OUT` | inserts a bibliography before body item N (the end when N is the item count) |
| `update FILE [--default-locale N] -o OUT [--json]` | rebuilds the results and prints the report |

A citation `SPEC` is a tag with optional switches after commas:
`--source Smith20 --source "Doe19,pages=12-14,volume=2,prefix=see ,suffix=,no-author,no-year,no-title"`.
A tag holding a comma is refused by the CLI (Python takes any tag).

Source JSON, the same shape as `sources --json` prints:

```json
{"tag": "Smith20", "kind": "journal_article", "guid": "", "locale": 1033,
 "contributors": [{"role": "author", "people": [{"last": "Smith", "first": ["John"], "middle": []}]},
                  {"role": "editor", "corporate": "WHO"}],
 "fields": [["title", "A study"], ["year", "2020"], ["journal_name", "Liver"]]}
```

`guid` and `locale` may be left out; `fields` is ordered and may repeat a
field.

### Python: `oxoffice.docx`

```python
Document.bibliography_sources() -> tuple[BibliographySourceInfo, ...]
Document.add_bibliography_source(source: BibliographySource) -> str  # the GUID
Document.replace_bibliography_source(source: BibliographySource) -> None
Document.remove_bibliography_source(tag: str) -> None
Document.bibliography_style() -> BibliographyStyleInfo | None
Document.set_bibliography_options(style: str, *, locale: int | None = None,
                                  locale_filter: int | None = None, tags: Sequence[str] = ()) -> None
Document.insert_citation(position: StoryRunPosition, sources: Sequence[str | CitationSource],
                         *, locale: int | None = None) -> None
Document.insert_citation_after_text(text: str, sources: Sequence[str | CitationSource],
                                    *, occurrence: int = 0, locale: int | None = None) -> None
Document.insert_bibliography(destination: int | StoryItem | Story, style: str, *,
                             locale: int | None = None, locale_filter: int | None = None,
                             tags: Sequence[str] = ()) -> None
Document.update_bibliography(*, default_locale: int | None = None) -> BibliographyUpdateReport

BibliographySource(tag, kind, *, guid="", locale=None, contributors=(), fields=())
BibliographyPerson(last, first=(), middle=())        # str or sequence of str for each part
BibliographyContributor(role, *, people=None, corporate=None)  # exactly one of the two
CitationSource(tag, *, pages=None, volume=None, prefix=None, suffix=None,
               suppress_author=False, suppress_year=False, suppress_title=False)
```

`fields` is a sequence of `(field, value)` pairs. Kinds, roles, fields and
styles are the snake-case names, typed as `Literal` in `_native.pyi`. The
info and report classes are frozen and compare by value, like the other
snapshot classes. Errors are the facade's, raised as `ValueError`.

## Plan

Each step is one pull request. Each is checked by behaviour tests named after
what they show, and by clippy on the crates touched.

1. **Tests for what exists**: `tests/docx/bibliography.rs` covers the source
   edits (identity, refusals, verbatim imported XML), the options, insertion,
   APA's 17 kinds and Book in the 11 other styles, and that an uncovered
   field fails the whole update. The new error message lands here.
2. **The Word oracle**: the probe generator (an example in `oxoffice-docx`),
   the Mac script that updates every field and saves, the 12 fixtures under
   `crates/oxoffice-docx/tests/docx/bibliography/` (a few tens of KiB each),
   and the comparison test. It holds the pairs not covered yet in its known
   list, which each step below empties.
3. Chicago and Turabian.
4. MLA 7th and Harvard - Anglia.
5. IEEE and ISO 690 numerical (numbered references, bibliography as a table
   for IEEE).
6. ISO 690 first element and date, GB7714 and SIST02.
7. GOST name sort and GOST title sort.
8. The Rust names and `add_bibliography_source`'s GUID, then the CLI
   commands, with tests in `crates/oxoffice-cli/tests/docx_bibliography.rs`.
9. The Python bindings and stubs, with tests in
   `crates/oxoffice-py/tests/test_docx_bibliography.py`.
10. README row, docs, the limitation file and the follow-up issue.

Steps 3 to 7 change `bibliography/` only, and each closes its rows of the
oracle's known list. Steps 8 and 9 build on the facade's signatures, which
steps 3 to 7 do not change, so they can run in parallel with them.
`insert_citation_after_text` lands in step 8.

## Outcome

Stopped after step 2 by the maintainer's decision (2026-11-23): steps 3 to
10 are not done, and an issue will be opened if the features are needed.

- Step 1 ([#578](https://github.com/oxoffice-dev/oxoffice/pull/578)):
  behaviour tests, and every refusal names its style, source and locale.
- Step 2 ([#589](https://github.com/oxoffice-dev/oxoffice/pull/589)): the
  Word oracle, with APA fixed to match it. Word for Mac 16.115 ships exactly
  our 12 styles, but it renders a document in the style set in the
  application, not the file's, and the maintainer ruled out macros and
  manual passes. Only APA 6th in en-US is therefore recorded and checked;
  of its 60 fields, all match Word except a patent entry's trailing space,
  and the entry order differs for a case and a person sharing a name.
- The limitation `docx-bibliography-word-check` and the README row say
  what is covered and checked.

## Links

- [#468](https://github.com/oxoffice-dev/oxoffice/issues/468), roadmap item 5.
- [#150](https://github.com/oxoffice-dev/oxoffice/issues/150): simple fields in tracked insertions.
- [Code inspection and refactoring](2026-11-21-code-inspection-and-refactoring.md): the split of `bibliography.rs`.
