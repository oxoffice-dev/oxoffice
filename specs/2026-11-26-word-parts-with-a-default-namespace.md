---
date: 2026-11-26
status: accepted
scope: docx
---

# A Word part written with the default namespace can be edited

## Context

A Word document whose root binds WordprocessingML as the default namespace
(`<document xmlns="http://schemas.openxmlformats.org/wordprocessingml/2006/main">`,
its elements without a prefix) opens, reads and saves unchanged, but every
edit is refused ("shadowed `default` namespace"), and in a debug build a save
reads back other ids than the model holds. The `default-namespace` writing of
#658 finds it on every corpus copy: 46 refused edits, 48 debug-only id
differences on 24 inputs (three `*.docx` entries of
`tests/corpus/writings-known.toml`). No producer of the corpus writes this
form; the writings spec names .NET `XElement` writers. Limitation
`docx-default-namespace`, #663.

Causes, found at a72aa528 with a local prototype (section 1 of the
decision, and `root_prefix` falling back to `w`; the diff in
`handoff/xcw/663-experiment.diff`):

1. **The main part's root writer drops the default namespace.**
   `CT_Document::from_xml` skips `xmlns` with the declarations "we always
   emit ourselves" (`oxoffice-docx-oxml` `document.rs:967`), but
   `CT_Document::root_start` (`document.rs:1407`) writes `xmlns:w`, `r`,
   `mc`, `wp` and the other captured declarations, never `xmlns`.
   `CT_HdrFtr` does the same (`header_footer.rs:256`). Every element the
   model keeps raw is written back as read, unprefixed, so a re-serialized
   part puts it in no namespace: in `libreoffice/contract.docx` the canonical
   form of `CT_Document::from_xml(part).to_xml()` reads `{}bookmarkStart`
   where the part says `{W}bookmarkStart`.
   - **The refusal** guards exactly that loss: `unsafe_serializer_namespace_prefix`
     (`oxoffice-docx` `document/namespaces.rs:210-233`) answers `default`
     when the root's default namespace is used, and every edit and modified
     save is refused (`content.rs:148`, `namespace_replay.rs:19`,
     `flush.rs:698`). The edit path itself is safe: it writes `w:` and binds
     it (the root writer always declares `xmlns:w`; an element written in
     place declares `w` when its part does not bind it, as the
     `other-prefixes` writing proves).
   - **The debug id difference** is the same loss: `IdentifierSummary::of`
     (`document/canonicalize.rs:518`) reads the ids of `document.to_xml()`,
     `of_package` those of the part as held; the raw bookmarks are missing
     on one side. A release build keeps the part byte for byte on a save
     without an edit, so only debug builds show it.
2. **The comparison reads a story with one prefix bound.** With 1 fixed and
   the refusal lifted, compare stops on the sample:
   - `root_prefix` (`comparison/namespaces.rs:5`) looks for `:{local}` as
     text: "comparison hdr root has no prefix" on every input with a header.
   - The text-box scans wrap a fragment in a root that binds only
     `xmlns:{word_prefix}` (`comparison/text_boxes.rs` 42, 172, 279; `"w"`
     hard-coded at 136, 223, 343, 884): an unprefixed `txbxContent` is not
     found, and the three `newsletter.docx` copies (authored, Word,
     LibreOffice) take the whole-box path and its #666 refusal.
   - Retained property XML is canonicalized with no binding or with the
     element's own prefix only (`comparison/paragraph_properties.rs`
     `retained_canonical` 431, `retained_xml_as_said` 479-528): an unprefixed
     `cnfStyle` against the model's `w:cnfStyle` reads as a formatting change
     (`word/manual.docx`, one diagnostic).
   - `without_own_val` (same file, 403) removes the `val` "under the
     element's own prefix": for an unprefixed carrier, the unprefixed `val`
     (no namespace: the model does not read it and the save keeps it), while
     the carrier's `w:val`, which the model decides, stays (#663's comment).
3. **A carrier finds its WordprocessingML attributes under `w`, its own
   declarations or its own prefix** (`oxoffice-docx-oxml` `properties.rs:465`
   `carrier_word_prefixes`, used by `modeled_toggle_attributes`). Under a
   default namespace the element has no prefix, and its attributes carry
   whatever prefix the part binds; the harness's writing keeps `w:` there,
   so only a document whose attributes use another prefix shows it. A probe
   of 14 operations (edits, notes, tables, lists, styles, comments, append,
   accept and reject, HTML, PDF) on five inputs, each in the harness's form
   and in that form (`other-prefixes` then `default-namespace`: elements
   unprefixed, attributes `ox4:`, `w` unbound), found one difference: after
   any edit of `word/report.docx`'s body, a paragraph's `cnfStyle` is
   written with `w:val` twice, which is no namespace-well-formed XML. (A
   table row's and a cell's `cnfStyle` lose their region attributes too, but
   in every writing, `w:` included: the table property model keeps `w:val`
   only. Not this class.)

Measured with the prototype (1 fixed, the refusal lifted, `root_prefix`
falling back to `w`): the PR sample passes every step of the writing in a
debug build; the full checks on the `default-namespace` writing alone leave
the four compare differences above; the pptx harness on that writing, full
checks, finds none (every deck part the writing rewrites reads, saves and
edits as its prefixed writing).

## Decision

### Options

- **A. Keep the default namespace as read, write new markup under `w`**
  (chosen). The root readers keep `xmlns` with the other declarations and
  the root writers write it back; raw XML keeps its writing and its
  namespace. What the model writes is prefixed, as everywhere (fixed on
  write). The refusal goes. The readers that tie an attribute's prefix to
  its element's, and the comparison's one-prefix reads, learn the part's
  bindings.
- **B. Normalize on open**: rewrite a default-namespace part to a prefixed
  writing in memory, write the original bytes back when nothing changed it.
  Not chosen: the package would hold other bytes than the file, against the
  verbatim rule, and in-place edits, the kept-part caches, signatures and
  every write path (save, package class, encryption, signing) would have to
  know which bytes are which.
- **C. Write with the part's own prefix, possibly none.** Not chosen: every
  writer that formats `{prefix}:{local}` (about 56 lines name the Word
  prefix in the comparison alone, and the property writers'
  `FieldXmlStart::word`) needs an empty-prefix path,
  and an unprefixed element is only right where nothing between it and the
  root rebinds the default namespace. Word reads any prefix.
- **D. Bind `w` at the root when an edit needs it.** Already done: it does
  not keep the default namespace that raw XML relies on.

### 1. The roots keep their default namespace

`CT_Document` and `CT_HdrFtr` keep an `xmlns` declaration among their
captured declarations and write it back after the canonical ones. The other
Word part readers map `xmlns` to the empty prefix already (styles,
numbering, comments and their extensions, settings, font table, glossary),
and the probe's notes, comments, styles and list edits give the prefixed
writing's result in both forms; the implementation checks each root writer
all the same.
`unsafe_serializer_namespace_prefix` loses its default-namespace branch and
`root_default_namespace_is_used` goes; `shadowed_namespace_diagnostic` loses
the #663 text. A default namespace declared on `w:body` or below keeps
today's handling (body declarations refused, nested owners replayed): no
writing or producer declares one there.

### 2. Attributes are WordprocessingML's by namespace

A retained carrier's WordprocessingML attributes are those whose prefix is
bound to WordprocessingML where the reader read it. A carrier written
without a prefix declares, when it is captured, the default namespace and
the prefixes its reader had bound to WordprocessingML that it uses
(`properties::carrier_bindings`); `carrier_word_prefixes` reads them as it
reads any declaration on the carrier. A carrier with a prefix is captured
as today. An unprefixed attribute never is a Word one (it has no
namespace). When the modelled element is written, the carrier's
declarations of WordprocessingML are not copied: its Word attributes are
written under the modelled element's prefix.

### 3. The comparison reads a story with its part's bindings

- `root_prefix` reads the root's name with a reader (`xml_element`), and
  returns the prefix new markup is written under: the root's own, else a
  prefix the root binds to WordprocessingML, else `w` (declared where it is
  written by `close_content_fragment_namespaces`, as for generated `w:`
  today).
- The wrappers that scan a fragment for WordprocessingML elements (text
  boxes, private markers, a paragraph's runs) bind WordprocessingML as the
  default namespace beside the Word prefix (`scan_root_start`): an
  unprefixed element there is the default namespace's of a part written
  with it. They look for Word local names (`txbxContent`, `r`,
  `fldSimple`) and private markers, which no other vocabulary of a Word
  part writes, so the part's own default need not be threaded to them.
- Retained property XML is canonicalized as today: an unprefixed carrier
  declares its bindings itself (section 2).
- `without_own_val` removes the carrier's WordprocessingML `val`, whatever
  its prefix (`carrier_word_prefixes`); an unprefixed `val` is kept and
  compared.

### What a save keeps of the input writing

- Without an edit: every part byte for byte, as today.
- An edit in place: the edited element written under `w:`, every other byte
  as read.
- An edit that writes the main part again: the root as the model writes it
  (`w:document`, the canonical declarations, then the producer's, `xmlns`
  among them: `with_producer_root` keeps the producer's root only when it
  reads the same), modelled elements under `w:`, raw elements unprefixed as
  read, in the default namespace the root still declares.
- A redline: its new markup under the root's Word prefix, `w` when the root
  has none. Same infoset as the prefixed writing; the harness holds the
  saved parts to the canonical form.

### Not done

DrawingML or PresentationML as the default namespace below a part's root
(inside a slide's shapes, a drawing in a Word part): no writing of the
harness and no producer seen; the pptx readers pass on the root's
namespace. A default namespace declared below a Word part's root (section
1).

## Plan

One pull request, `fix(docx): a word part written with the default
namespace can be edited`, closing #663:

1. `docx-oxml`: the two roots keep `xmlns`; carriers read their attributes
   by namespace (section 2).
2. `docx`: the refusal and its helper removed; the comparison changes of
   section 3.
3. The limitation `crates/oxoffice-cli/limitations/docx-default-namespace.json`
   and the three `*.docx` `default-namespace` entries removed;
   `docs/ARCHITECTURE.md`, model conventions: a part's root keeps its
   declarations, the default namespace included, and raw XML its writing.

Checked by `cargo test -p oxoffice-docx-oxml -p oxoffice-docx`, the writings
sample, the full checks of the docx harness (an entry that does not happen
fails, so they prove the three entries gone and the four compare
differences fixed) and the compare walks with `OXOFFICE_WORKFLOWS=full`.

### Tests

- The harness: its three entries removed. The PR sample then runs compare,
  accept and reject on the `default-namespace` writing (today it stops at
  the refused edit), and its debug save checks the ids.
- `a_kept_paragraph_property_counts_only_when_what_it_says_changes`
  (`tests/docx/equivalent_writings.rs`, the table of #676) runs each row in
  four writings instead of two: as written, `other-prefixes`,
  `default-namespace`, and `default-namespace` over `other-prefixes` (the
  attributes' prefix bound neither to `w` nor to the element). Its rows
  already hold a table paragraph's `cnfStyle` with a region attribute and
  the toggle carriers, edited, saved and compared: without sections 1 to 3
  the new writings fail on the refusal, the carriers' `w:val`, the
  canonical `cnfStyle` and the doubled `val`. One closure and one array in
  the driver. The table's row and cell are written again by the edit
  already; a `cnfStyle` there would fail in every writing alike (cause 3's
  note), so the rows do not hold one.
- `a_header_written_again_keeps_its_default_namespace`: a header with a
  bookmark around its paragraph (held raw), in the default namespace,
  written again by `replace_text`, reads as its `w:` writing does. No other
  test writes a header's model again under that writing.
- `main_part_root.rs` asserted the old writer: a root declaring a default
  namespace was written anew after an edit. It now stays as the producer
  wrote it (`a_root_with_a_default_namespace_is_kept`).
- No Python test: the bindings are not touched.

## Decided with the maintainer

1. Option A.
2. The form with attributes under another prefix is tested by the #676
   table only, not walked by the harness (one more writing costs a harness
   pass on every input); the probe found no other reader that ties an
   attribute's prefix to its element's.
3. Word's reading of the mixed writing (a `w:document` root that also
   declares the default namespace, raw children unprefixed), the same
   infoset under the Namespaces in XML recommendation, is checked in Word
   later, not as a merge gate.
4. The table rows of the #676 test are edited by an operation that writes
   the row again (`replace_text` does).

## Links

- #663, #658, #676, #666
- [Equivalent XML writings give the same result](2026-11-25-equivalent-xml-writings.md)
- Prototype diff and probe: `handoff/xcw/663-experiment.diff`,
  `handoff/xcw/663-probe.rs` (outside the repository)
