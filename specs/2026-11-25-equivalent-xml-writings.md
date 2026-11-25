---
date: 2026-11-25
status: draft
scope: tests, docx, pptx
---

# Equivalent XML writings give the same result

## Context

The same XML can be written many ways, and each producer picks its own:
an empty element as `<x/>` or `<x></x>`, a namespace under any prefix or as
the default namespace, whitespace between elements, attributes in any order
and either quote, a character as itself or as a reference. A reader that
parses the XML sees one infoset whatever the writing. Code that reads XML
as text sees one writing, the one its author had at hand, and fails on the
next producer's.

Each writing has been found by a user so far: #656 (Google Docs leaves
empty inline content controls written `<w:sdtContent/>`: compare refused
any edited paragraph holding one), #413 (LibreOffice's table of contents
control). `src` holds 11 hand-written close-tag searches (`rfind("</...>")`:
8 docx, 2 pptx, 1 docx CLI) and about 19 "no end tag" errors, and more
searches through a variable (`xml.rfind(&closing)`). The suite reads each
producer's file as that producer wrote it, never another writing of it.

## Decision

### 1. The writings

A dev-only crate, `oxoffice-xml-test` (never published, depending on
neither facade, like `oxoffice-render-test`), rewrites a part one way at a
time and copies every other byte as it was, so that a failure points at one
writing:

| Writing | What changes | Seen with |
| --- | --- | --- |
| `empty-with-end-tag` | every `<x .../>` as `<x ...></x>` | LibreOffice (`w:fldChar`, #413) |
| `empty-self-closed` | every `<x ...></x>` with nothing inside as `<x .../>` | Google Docs (`w:sdtContent`, #656) |
| `other-prefixes` | the main namespaces (WordprocessingML, PresentationML, DrawingML main, relationships, math, wordprocessingDrawing, picture, chart, diagram; transitional and strict) bound to `ox0`, `ox1`... where their prefixes were declared | generated writers (`ns0:`) |
| `default-namespace` | the root's namespace as the default namespace: its elements without a prefix, its attributes keeping theirs (an attribute without a prefix has no namespace) | .NET `XElement` writers |
| `indented` | a line break and an indent between tags, never in an element without child elements, in an element with text of its own, or under `xml:space="preserve"` | pretty printers |
| `attribute-order` | the attributes of every tag, namespace declarations included, in reverse order | any writer |
| `single-quotes` | every attribute value between `'` | any writer |
| `tag-spacing` | line breaks and spaces inside tags: around `=`, between attributes, before `>`, `/>` and the `>` of an end tag | hand edits |
| `character-references` | every character outside ASCII as `&#x..;`, `>`, `"` and `'` as entities, in text and attribute values | ASCII-only writers |
| `leading-character-reference` | the first non-space character of every text and of every attribute value that declares no namespace as `&#..;`, ASCII letters and digits included | none known: the worst case of the one above |

The markup compatibility attributes that list prefixes (`mc:Ignorable`,
`mc:MustUnderstand`, `mc:ProcessContent`, `mc:PreserveElements`,
`mc:PreserveAttributes`, `Requires` on `mc:Choice`) follow a renamed
prefix. Whitespace and references to whitespace are never written, so no
attribute value changes under attribute-value normalisation.

Not done: CDATA sections, comments and processing instructions added,
line ends, the XML declaration, other encodings. No producer of the
corpus writes them; each is one more `Writing` when one does.

### 2. Each rewrite proved equivalent

Every part a rewrite changes must read the same as the original in a
canonical form, or the rewrite is refused (the test fails on the harness,
not on the library). The canonical form is read by other code than the
rewrites (quick-xml's attribute reader, its own namespace scopes): one line
per element start and end, the name expanded to `{namespace}local`, the
attributes sorted by expanded name without namespace declarations, values
with references resolved and literal whitespace normalised; one line per
text, references resolved and CDATA read; an empty-element tag as a start
and an end. A text of whitespace only, outside `xml:space="preserve"`, in
an element that holds elements is no line: WordprocessingML, PresentationML
and DrawingML declare element-only content there. The prefixes the markup
compatibility attributes list are expanded. Two writings are equivalent
when their canonical forms are equal; this is the infoset minus namespace
declarations, prefixes and that whitespace.

### 3. Which parts

Every XML part of the WordprocessingML, PresentationML and DrawingML
vocabularies: the main document, styles, numbering, settings, font table,
headers and footers, footnotes and endnotes, comments and their extensions,
themes, charts and their styles, glossary, the presentation, slides,
layouts, masters, notes slides and masters, table styles, presentation and
view properties. Not the package's own parts (`[Content_Types].xml`,
relationships), the document properties under `docProps/` or the custom XML
data under `customXml/`: other readers parse them (the OPC crate), and a
separate writing can be added for them. One rewrite changes every such part
of a package at once.

### 4. The operations and what "the same result" means

The operations run on the original and on each rewrite; each step runs
only when the one it needs passed.

- **Word documents** (`crates/oxoffice-docx/tests/docx/equivalent_writings.rs`):
  open; validate (the stored bytes and the live document, their findings);
  read the text of every story (`story_paragraphs`: kind, style, text) and
  the Markdown of the body (headings, lists, tables, bold, italic); save
  without an edit and open again; edit one paragraph of each kind of story
  and the first after each quarter of the body (`try_replace_text_at`, a
  word of four letters or more found once in it, in the other case), save
  and open again; compare the original with the edited copy, save the
  redline and open it again; accept every change (gives the edited
  version's text) and reject every change (gives the original's), each
  saved and opened again.
- **Decks** (`crates/oxoffice-pptx/tests/pptx/equivalent_writings.rs`):
  open; validate (the live deck and the stored file); read the text and
  notes of every slide and the Markdown export with its diagnostics; save
  without an edit and open again; replace a word of each slide's text
  (`try_replace_text`), save and open again.

The same result is, step by step:

- the same outcome: both pass with the same text read back (texts,
  validation findings, comparison diagnostics, replacement counts), or both
  fail with the same message. A rewrite that passes where the original fails
  is a difference too;
- for each step that saves, every part of the saved package reads the same
  as what the original's step saved, in the canonical form: a writer that
  writes a canonical prefix the part does not declare, or drops what it does
  not recognise, shows here;
- a save without an edit keeps byte for byte every part it keeps byte for
  byte on the original, in the writing it was read in: the verbatim rule of
  the model conventions. A part the original's save rewrites already
  (`known_failure` in the corpus manifest) is only held to the canonical
  form.

### 5. Inputs, sample and budget

- Inputs: documents built by the tests (the shapes behind #656 and #413 in
  a few paragraphs, written as Word would write them; the small thesis; a
  deck from the default template) and the corpus copies.
- Pull requests run the built inputs and two Word copies
  (`google/form.docx`, `libreoffice/contract.docx`: the writings furthest
  from Word's, with comments, notes, tracked changes, fields and controls),
  the built deck and the deck of each producer, every writing. Measured on
  a 32-core machine in the `scaling` profile of the PR CI: 98 CPU seconds
  for the Word binary (10 s of wall time), 34 for the deck binary (3 s),
  so about half a minute of wall time on a 4-core runner.
- The full checks (`OXOFFICE_WORKFLOWS=full`, the switch of the workflow
  walks in `crates/oxoffice-docx/tests/docx/support.rs`) run every corpus
  copy, every writing, but the Word `long-*` copies: they repeat their
  family's markup and cost as much as all the other Word copies together.
  Measured: 3,556 CPU seconds for the Word binary in the `scaling` profile
  (about twice that under the debug assertions of the `ci` profile the full
  checks use), 332 for the deck binary.
- `OXOFFICE_WRITINGS_ONLY="google/form.docx indented"` narrows a local run
  to the inputs and writings named.

### 6. Known differences are strict

`tests/corpus/writings-known.toml` lists what differs today: an input (or
`*.docx`, `*.pptx` for a class met on many inputs), a writing, an
operation, the issue, and text the finding's message must hold. As
everywhere in the suite, an entry is strict: a difference not listed fails
the test, and an entry that does not happen fails it too, so that a fix
removes its entries. An entry for one input is checked whenever its input
and writing run; one for many inputs by the full checks only. A debug
build's cross-checks can fail before the step itself does (a save that
reads back other ids than it holds): such an entry says
`debug_assertions = true`, and the one it hides in a build without them
`false`. Entries are grouped by root cause, each with the code it points
at; they cite #658 until their class has an issue of its own.

### 7. The class fix (phase 2)

- One shared helper for the inner content of an element, an element
  written self-closing included (an empty range at the end of its tag),
  and one to write content into it (the owner gets an end tag when content
  is written, keeps its writing when none is): the comparison's
  `element_inner_range_any_prefix` and `replace_owner_inner` (#656) move to
  `oxoffice-core`, and every close-tag search of docx, pptx and the docx
  CLI goes through them.
- A check refusing new hand-written close-tag searches outside that helper
  (`find`/`rfind` of a string starting `</`, literal or formatted): a test
  that walks `crates/*/src`, with the few searches of our own output (HTML,
  EPUB) listed by file.
- The other classes the inventory finds: fixed in the same pull request when
  small, otherwise one issue per root cause, each with its entries in the
  known list.

## Plan

1. This pull request, phase 1: the crate, the two test modules, the known
   list, this spec. The inventory of what fails, by class, is in the pull
   request.
2. Phase 2, same pull request, after #656: the shared helper and the check,
   the classes agreed in the inventory fixed, their entries removed.

## Links

- #658, #656, #413
- [The test suite from scratch](2026-11-20-test-suite-from-scratch.md),
  [Corpus at scale](2026-11-21-corpus-at-scale-and-behaviour-tests.md),
  [Consumer workflows](2026-11-21-consumer-workflows.md)
