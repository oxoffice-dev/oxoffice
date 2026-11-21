---
date: 2026-11-21
status: accepted
scope: pdf, docx, cli, python
---

# Fillable PDF forms

## Context

ROADMAP item 4, from [rdocx#381](https://github.com/tensorbee/rdocx/issues/381).
A Word template with form controls exports to a PDF where the controls are
plain page content: the recipient cannot fill it in or sign it. Word cannot
do it either, its PDF export flattens forms.

What exists today:

- **Model.** Content controls (`w:sdt`) are parsed at block, row, cell and
  inline level, with their type (plain text, rich text, checkbox, combo box,
  drop-down list, date, picture, groups...).
  - Tag, alias and id are typed.
  - Lock, placeholder, list items, checkbox states and date format sit in raw
    slots. `CT_SdtPr::checkbox_state` and `prepare_text_value` already decode
    some of them.
  - Legacy fields (`FORMTEXT`, `FORMCHECKBOX`, `FORMDROPDOWN`) are parsed
    into `LegacyFormFieldData`, without their help text or text input type.
  - `w:documentProtection w:edit="forms"` is modelled, `w:formProt` is not.
- **Layout.** A control's identity is lost: block controls become ordinary
  paragraphs, and inline control runs merge into the paragraph's runs.
  Nothing answers "where did this control land". Hyperlinks are the one
  precedent: a URL rides on each text segment through line breaking, and the
  paginator pushes one `LinkAnnotation` rectangle per segment per line.
  - Two persistent engines reuse paragraph blocks across edits. A paragraph
    block is cached on the paragraph's XML and geometry, and its source nodes
    are relabelled on reuse (`remap_sources`).
  - The only revision views are accepted and tracked.
- **Rendering.** Placeholder text is drawn like any text. A Word-saved
  `FORMCHECKBOX` has no stored result, so nothing is drawn for it.
- **PDF.** `oxoffice-pdf` writes link annotations only, with the Print flag
  only under a PDF/A profile. Tagging is automatic when the layout carries a
  structure, which Word layouts always do. `pdf-writer` 0.15 already has
  AcroForm and field writers.
- **Surface.** `RenderOptions` (Copy) is the facade's only render options
  type. The CLI (`docx convert --to pdf`) and Python (`to_pdf`,
  `to_pdf_with_diagnostics`, `to_pdfa_deterministic`) build it and call the
  layout and `render_to_pdf_checked` themselves. PowerPoint shares the PDF
  entry points.
- **Tests.** No PDF output test exists. No PDF reader in tests, no veraPDF in
  CI. `lopdf` 0.44 is a workspace dependency (used by `oxoffice-pptx`). The
  corpus spec plans a `form` family (content controls, legacy fields,
  protection), not produced yet.

## Decision

An opt-in switch turns the Word form controls of the main body into AcroForm
fields at their laid-out positions. Without the switch, this work changes no
output, byte for byte. Pull request 3 is a separate rendering fix: it changes
the static output of Word-saved legacy checkboxes on purpose, switch or not.

### Which control becomes which field

| Word | Field | Value |
| --- | --- | --- |
| plain text control | text (`/Tx`) | shown text |
| rich text control | text, plain value | shown text, formatting only in the first appearance |
| date picker | text | shown text, format not enforced |
| drop-down list | choice, combo (`/Ch`) | value of the shown item, `/Opt` as `[value display]` |
| combo box | choice, combo, editable | value of the shown item, or the typed text |
| checkbox (`w14:checkbox`) | check box (`/Btn`) | `/Yes` or `/Off` |
| legacy text field | text, `MaxLen` from `maxLength` | result text |
| legacy checkbox | check box | checked state |
| legacy drop-down | choice, combo | selected entry |
| a content control marked as a signature | empty signature (`/Sig`) | none |

- **Not mapped:**
  - picture controls, unless marked as a signature;
  - building block galleries, citations, bibliographies, equations.
  
  Groups and repeating sections are containers: their inner controls are
  mapped. A control that holds other mapped controls stays static, and its
  inner controls become fields.
- **Mapped control**, in the rest of this spec: a control of a mapped type
  that is not a container. Names, the count of fields and diagnostics, and
  the tests all use this meaning.
- **Block controls hold paragraphs only.** A block control around a table,
  and every row-level or cell-level control, is a container: never a field
  itself, its inner controls mapped. One field over a whole grid would wipe
  every cell's text on the first keystroke. So no control around a table,
  its rows or its cells marks the table. A block control inside a cell holds
  paragraphs, and is a field like any other.
- **Signature.** A plain text, rich text or picture control whose tag is
  `signature` or starts with `signature:` (ASCII case ignored) becomes an
  empty `/Sig` field the recipient can sign in Acrobat Reader. Legacy fields
  never do: their names allow no `:` and must be unique. The convention is
  user documentation, not only spec: the README's PDF section, the CLI help
  of `--fillable-forms` and the Python docstring state it.
- **Name (`/T`).**
  - Tag, else alias, else legacy field name, else the kind (`text`,
    `checkbox`, `list`, `date`, `signature`).
  - `.` becomes `_`, since it separates levels in AcroForm names.
  - A duplicate takes the next free suffix (`_2`, `_3`...), skipping any
    name already taken.
  - Names come from the model, in document order, over every mapped control
    of the exported stories, never from the laid-out fragments. So they
    depend on the document only, never on fonts or pagination.
- **Tooltip (`/TU`).** Alias, else legacy help text, else the control's own
  text while it shows its placeholder (no glossary lookup). In tagged output
  the name is the last fallback, so every field has one.
- **Flags.**
  - Read only when the control's content is locked (`contentLocked`,
    `sdtContentLocked`), the legacy field is disabled, or a legacy text input
    is computed (`currentDate`, `currentTime`, `calculated`).
  - Multiline when `w:text w:multiLine` is set, when the control is block
    level with several paragraphs or line breaks, or when the layout wraps
    it onto several lines.
  - Combo for lists, plus Edit for a combo box.
- **Placeholder.** Showing placeholder text means the field has no value,
  and its appearance is empty, so a printed blank form is blank. The
  placeholder text becomes the tooltip. The layout output says so with
  `FormField::placeholder`. The fragment's children still hold the
  placeholder drawing, for the raster and the flattening invariant.
- **Default appearance (`/DA`).** Font size and colour of the control's
  first run, or, for a control with no runs, of its paragraph mark.
- **Alignment (`/Q`).** From the paragraph: start, centre or end, mirrored
  in a right-to-left paragraph. Justified counts as start.
- **Revision views.** Fields follow the accepted view, where the value is
  the control's accepted text, and a deleted control has no field.
  - A block control that the accepted view joins with outside text, across
    a deleted paragraph mark, stays static.
  - The tracked view refuses the switch, since a value would mix inserted
    and deleted text. In Rust that is an error like the refused system-font
    request (`Error::Other`). Python raises `ValueError` before calling
    Rust, as for its other option clashes.
- **Data binding.** Each control is its own field, even when two controls
  share a binding. Sharing a value is left for later.

### Where a field goes on the page

- **Exported stories.** The main body, tables included at any depth.
  - Headers, footers, notes, comments and text boxes stay static: header
    controls repeat on every page, and text boxes may be rotated.
  - Controls in rotated table cells (`tbRl`, `btLr`) and in vertical
    sections (`w:textDirection`) stay static too, with a diagnostic.
- **Marking.** The Word engine marks every line item drawn for a control:
  text, tab, inline picture. The mark is local to the paragraph: the
  control's position among the mapped controls that enclose or sit in that
  paragraph. Together with the paragraph's source node, which the engines
  already relabel when they reuse a block, it identifies the control. A
  floating picture inside a control is not a line item, so that control
  stays static, with a diagnostic.
- **Fragments.** Where the paginator draws marked items on a line, it wraps
  what it draws (glyph runs, decorations, tab leaders, inline pictures) in a
  fragment, with that line's rectangle, like the link rectangle. It places
  the fragment inside its paragraph's marked content, as it does text, never
  as an artifact.
- **Cache safety.**
  - The switch joins the engines' block reuse check, as the hyperlink map
    does, and keys the facade's layout cache.
  - With the switch, a paragraph's cache key also holds how many mapped
    block controls enclose it, so identical paragraphs inside and outside a
    control never share a block.
  - Marks follow source nodes through `remap_sources`.
- **Numbering.** After pagination, a pass resolves each fragment's (source
  node, mark) to its control in the model, numbers the fields in document
  order, and builds `form_fields`.
- **Rectangle.** The writer unions a field's fragments on its page into the
  widget rectangle (transforms applied, as for links). A text, list or
  signature control alone in its paragraph, or block level, gets the full
  text width of its paragraph (indent to indent, the cell width in a
  table), so there is room to type. A check box keeps its glyph box.
- **Empty controls.** A control with no runs (empty, without placeholder)
  still gets a zero-width anchor on its line, the way field targets keep a
  position, and an empty fragment. Alone in its paragraph, it takes the full
  width as above. Inline among text, it takes a minimum width of 6 em of its
  paragraph's font from the anchor, which can overlap the text after it
  (diagnostic).
- **Splits.** A control the layout splits across pages or columns stays
  static, with a diagnostic. An inline control that wraps onto several lines
  becomes a multiline field over the union of its lines. Text before it on
  its first line, and after it on its last, can then sit under the box
  (diagnostic).
- **Nothing lost silently.** Every mapped control of the exported stories
  that ends without a field gets a diagnostic naming it: dropped by the
  view, split, rotated, holding a floating picture. The tests count fields
  plus these diagnostics against the mapped controls. Containers (groups,
  repeating sections, controls around tables, rows or cells, and controls
  holding other mapped controls) are not mapped controls, so they are
  neither named nor counted.

### What the fields look like

- **Appearance streams from the layout.** Each widget's normal appearance
  (`/AP /N`) is what the layout drew for the control: the fragment's
  children, with the document fonts already subset and embedded. No
  `NeedAppearances`.
- **Drawn once.** The fragment's children leave the page content: they are
  drawn only in the appearance. Otherwise a new value typed over a
  pre-filled one would sit on top of the old one, which could not be
  removed.
- **Same look, except empty fields.** Until a field is edited, the PDF looks
  like the static export, except where a field shows its placeholder
  (`placeholder`) or is a signature: there the appearance is empty, by
  decision, while the children still leave the page.
- **Printed.** Every widget has the Print flag and no Hidden or NoView
  flag, PDF/A or not, since the values live only in the widgets.
- **Check boxes.** `/AP /N` holds `/Yes` and `/Off`. The current state comes
  from what the layout drew. For the other state, the layout shapes the
  other symbol (`w14:checkedState` / `uncheckedState`, ☐ and ☒ by default,
  which the bundled Noto Sans Symbols 2 subset covers) into the fragment's
  `alternate`.
- **Typing font.** The default resources (`/DR`) embed Liberation Sans
  Regular (bundled, with its OFL licence beside it, metric-compatible with
  Arial and Helvetica) as a simple TrueType font with WinAnsi encoding.
  - The whole font is embedded, since the `subsetter` crate drops the
    `cmap` a simple font needs.
  - That adds about 190 KB compressed to a PDF with fields, and covers every
    WinAnsi character a user may type.
- **What will not display.** Text typed in a script Liberation Sans lacks
  (Chinese, Japanese, Korean, Arabic, Hebrew, Devanagari, Thai...) depends
  on the viewer: some substitute a font, some draw nothing. Values already
  in the document in those scripts display correctly, since their appearance
  comes from the layout, until the field is edited. This is a known
  limitation, with its `limitations.json` entry and issue.
- **No JavaScript.** No `/A` or `/AA` on fields or widgets: PDF/A forbids
  them and many viewers ignore them. So date formats are not enforced, and
  nothing is calculated.

### Tagged and PDF/A output

- Each widget gets a `/Form` structure element, with an `OBJR`, a
  `/StructParent` and its parent tree entry. It is a child of the element of
  the marked content around its fragment (its paragraph), else of the
  document root.
- Pages with widgets get `/Tabs /S`.
- PDF/A: the appearance dictionary holds `/N` only, no actions, and the
  `/DR` font is embedded. The PDF/A preflight accepts form fields.
- Not fixed here: link annotations are not tagged under `/Link` today.
  (Since fixed: each link gets a `/Link` element the same way, a child of
  its paragraph's element, and pages with links get `/Tabs /S` too.)

### Format-neutral layout output

`oxoffice-layout` gains, with no Word words in it:

```rust
pub struct FormFieldId(pub u32);            // index into LayoutResult::form_fields

pub struct FormField {
    pub name: String,
    pub tooltip: Option<String>,
    pub kind: FormFieldKind,
    pub read_only: bool,
    pub font_size: f64,
    pub color: Color,
    pub alignment: FieldAlignment,          // Left, Center, Right
    pub placeholder: bool,                  // the drawing is a placeholder: empty appearance
}

pub enum FormFieldKind {
    Text { value: String, multiline: bool, max_len: Option<u32> },
    CheckBox { checked: bool },
    Choice { options: Vec<ChoiceOption>, value: Option<String>, editable: bool },
    Signature,
}

pub struct ChoiceOption { pub value: String, pub display: String }

pub struct FormFieldFragment {
    pub field: FormFieldId,
    pub rect: Rect,
    pub children: Vec<PositionedElement>,   // the static drawing of the value
    pub alternate: Vec<PositionedElement>,  // check box: the other state
}
```

- `LayoutResult::form_fields` (the struct is `#[non_exhaustive]`) and
  `PositionedElement::FormField(FormFieldFragment)` (also non-exhaustive).
- The Word engine's marks are internal to it. The numbering pass turns them
  into `FormFieldId`, so the shared types carry no Word identity.
- The writer writes an AcroForm when, and only when, the layout holds
  fragments. A layout emits them only on request, and PowerPoint never does.
  That is what keeps every other output unchanged.
- The producer names the fields. The writer only repairs a name that would
  break the PDF (empty, `.`, duplicate), the same deterministic way.
- **Traversals.** A fragment is a container, like `MarkedContent`. Many
  element traversals end in a catch-all arm, and the compiler flags none of
  them. Each pull request that adds or emits fragments lists the traversals
  and updates them:
  - in the Word layout: vertical page alignment (`translate_element_tree`),
    the shape text move, block measurement, page field substitution, the
    artifact wrapping, `remap_sources`, and the font passes
    (`collect_carrier_fonts`, `canonicalize_layout_fonts`), which must also
    visit `alternate`;
  - in the PDF writer: resource collection, alpha and gradient passes,
    structure marks, emission;
  - the raster and the SVG export, which draw the children;
  - `walk`, which visits the children, so subsetting sees the glyphs.
    Subsetting also visits `alternate`.
- **Invariant.** Replacing every fragment by its children gives exactly the
  Word layout made without the switch.

### API

- **Rust.** `RenderOptions::fillable_forms: bool` (default `false`) reaches
  `LayoutInput::form_fields`. Every `*_with_options` PDF call honours it, and
  so does `save_pdf_with_options`. PDF/A with fields goes through:
  - `to_pdfa_with_diagnostics(profile, fonts, options)`;
  - a new `to_pdfa_deterministic_with_options(profile, options)`, the twin of
    `to_pdf_deterministic_with_options`, which Python's
    `to_pdfa_deterministic` calls.
  
  The tracked view refuses the switch with an error.
- **One break, the last one.** The same change marks `RenderOptions`
  `#[non_exhaustive]`, so later options break nothing. Outside the crate, it
  is then built from `Default` and set with `with_*` methods or field
  assignment. Struct literals, including `..Default::default()`, no longer
  compile there. Hence `feat(docx)!`.
- **CLI.** `oxoffice docx convert form.docx --to pdf --fillable-forms`,
  refused with any other target and with `--revision-view tracked`.
- **Python.** `fillable_forms=False` on `to_pdf`, `to_pdf_with_diagnostics`
  and `to_pdfa_deterministic`, in the stubs too.
- Image rendering ignores the switch: it draws the static rendering.

### How we test it without Acrobat

- **PDF writer (Rust).** Synthetic layouts, read back with `lopdf` (new
  dev-dependency of `oxoffice-pdf`, already in the workspace):
  - field types, names, flags (Print on every widget), values, options,
    rectangles, `/Q`;
  - one appearance per state, both drawing their glyphs, `/DA` and `/DR`, no
    `NeedAppearances`, no actions;
  - a placeholder field and a signature have an empty appearance, and their
    children are not on the page either;
  - a fragment's children are absent from the page content stream;
  - tagged output: `/Form` under the right parent, `OBJR`, parent tree,
    `/Tabs`;
  - identical bytes on two runs, and no AcroForm without fragments;
  - the raster of a layout with fragments equals the raster of its
    flattened layout.
- **Byte identity without the switch.** A script renders to PDF on `main`
  and on the branch, compared byte for byte, its result in the pull request:
  - the docx and pptx corpus;
  - the forms fixture, written once by the branch, since the corpus has no
    form controls yet.
- **Word layout and facade (Rust).** A fixture built inside the test with
  the facade's own insertion calls:
  - every control type, legacy fields (a computed one too), a
    signature-tagged text control and a signature-tagged picture control, a
    locked control, one showing its placeholder;
  - empty ones, alone in a paragraph and inline, a right-aligned one, and one
    in a right-to-left paragraph;
  - a block control next to a plain paragraph with the same text, and a
    block control around a table;
  - one in a table cell, one in a rotated cell, one split across pages, one
    split across columns, one in a header, one in a vertically centred
    section;
  - one deleted in a tracked change, and one joined with outside text by a
    deleted paragraph mark.
  
  The tests assert on `form_fields` and fragments (page, a rectangle that
  holds the control's drawing and stays inside the text area), and on the
  PDF read back with `lopdf`. They also check:
  - the flattening invariant;
  - fields plus "no field" diagnostics account for every mapped control of
    the exported stories, one field or one diagnostic each;
  - the same `Document` laid out with the switch, then without, gives no
    fragment the second time, and with it again gives the same fields;
  - after an edit (a control inserted early in the body, a font change that
    makes the reusable engine canonicalize its fonts), every name, value and
    check box glyph stays with its own control;
  - the tracked view refuses the switch.
- **Python.** Both libraries join `.github/requirements/ci.in` with hashes.
  - `pypdf` lists the fields (names, types, values, rectangles, flags). It
    finds none of the fixture's field values in the page text, except the
    plain paragraph that deliberately repeats one block control's text,
    where that text appears exactly once. It fills new values, toggles the check boxes, saves,
    and reads them back, which shows the PDF really is fillable.
  - `pypdfium2` (PDFium, the engine of Chrome) renders the page with forms
    drawn and compares it with the static export, within a small pixel
    tolerance, outside the bounds of the children of placeholder and
    signature fields, which must be blank.
  - It renders again with forms off, and the bounds of the fragments'
    children are blank. Those bounds, not the widget rectangles, which can
    cover neighbouring text.
  - Every widget has the Print flag and neither Hidden nor NoView: a
    bitmap render in pdfium's print mode does not honour the Print flag,
    so the flags are checked instead of a print-mode render.
  - After new values are filled and check boxes toggled, it renders once
    more: the old value's pixels are gone, and each box shows its other
    symbol.
- **veraPDF.** A Linux CI job (Java is on the runner) exports the fixture
  with and without the switch, then checks PDF/A-2b and PDF/UA-1. Today's
  tagged output is not checked yet, so the bar is relative:
  - with the switch, no failure in the annotation and form clauses (PDF/A-2
    6.3 and 6.4, PDF/UA-1 7.18);
  - no rule fails more checks with the switch than without;
  - the job fails when a report is missing or unreadable.
  
  veraPDF is downloaded at a pinned version and checked against its SHA-256.
- **Corpus.** When the `form` family lands (Word, LibreOffice, Google), a
  behaviour test exports every file. It checks one field or one "no field"
  diagnostic per mapped control of the exported stories, the expected names
  and values, and no panic.
- **By hand, before the roadmap item is closed.** Fill and save in Acrobat
  Reader and Preview, and sign the signature field in Acrobat Reader. Only
  the user can do this.

### Not now

- Signing (PAdES), signature appearances, `/SigFlags`.
- JavaScript: date format, number format, calculation, validation.
- Fields in headers, footers, notes, comments, text boxes, rotated cells and
  vertical sections.
  These are known limitations, with their issues, in the last pull request,
  like typing in non-Latin scripts.
- Rich text values (`/RV`), radio groups (Word has none), list boxes,
  shared values for data-bound controls.
- A WinAnsi subset of the typing font with an added `cmap`, if 190 KB
  matters.
- Importing a filled PDF back into the Word document.
- PowerPoint.

## Plan

Rust first, then the CLI and Python. Each pull request has its own check.

1. **`feat(pdf): acroform fields from the layout output`.**
   - Scope:
     - the layout types and `walk`;
     - the raster;
     - the writer: fields, appearances, check box states, choices,
       signature, `/DR` font, names, alignment, Print flag;
     - the writer's and raster's traversals, listed and updated.
   - Check: the PDF writer tests above except the tagged ones (pull request
     2), corpus byte identity, clippy.
2. **`feat(pdf): form fields in tagged and pdf/a output`.**
   - Scope: `/Form` elements, `OBJR`, parent tree, `/Tabs`, `/TU` fallback,
     and form fields accepted by the PDF/A preflight.
   - Check: `lopdf` tests on tagged and PDF/A synthetic layouts.
3. **`fix(docx): draw word legacy checkboxes`.**
   - Scope: a `FORMCHECKBOX` without a stored result draws ☐ or ☒ at its
     `ffData` size (or the run size when the size is automatic). This is a
     static rendering fix, needed before such checkboxes can become fields.
   - Check: a layout test (glyph and size), and a Word-saved checkbox
     rendered locally.
4. **`feat(docx): form facts of content controls and legacy fields`.**
   - Scope: in `oxoffice-docx-oxml`, the form facts of a control decoded
     from its slots (lock, placeholder, list items, checkbox states, date
     format, `multiLine`). The legacy help text and text input type are kept
     on read. The facade's private form reader is left as it is.
   - Check: unit tests on XML as Word, LibreOffice and Google write it.
5. **`feat(docx)!: word form controls as fillable pdf fields`.**
   - Scope:
     - in `oxoffice-docx-layout`: marks and anchors, fragments, the
       numbering pass, the placement rules, the traversals, the cache
       safety rules, `LayoutInput::form_fields`;
     - in the facade: `RenderOptions::fillable_forms`, `#[non_exhaustive]`
       and its `with_*` methods, the cache key,
       `to_pdfa_deterministic_with_options`, the tracked-view refusal, the
       SVG export's arm;
     - the docx CLI and Python crates move to building `RenderOptions` from
       `Default`, with no new surface.
   - Check: the Word layout and facade tests above, and byte identity with
     the switch off.
   - As built:
     - **Who marks.** Only a caller that knows a paragraph belongs to the
       main story passes its form identity to paragraph layout: the body
       by the paragraph's registry node, a table cell only under the
       registry the inventory used and the document story. Headers, notes,
       text boxes and cached copies draw no marks, so a sentinel or local
       source node can never name a body field.
     - **Markers, then a gathering pass.** The paginator leaves one
       non-drawing marker per marked item, over the box that item drew,
       like a link rectangle. A tab between two items of one control is
       marked too, so its leader is the field's, and so is a picture drawn
       as a figure. A line that draws one field only, with no list number,
       drawing group (bare or described) or outside leader on it, gives the
       widget its text band, from the line's own indent to its end. After
       layout, a pass gathers into the fragment what belongs to a marker's
       box: text that starts in it, unless its source names another
       paragraph (field results, symbols and capitalised text carry no
       source and still join, from anywhere), the very picture it stands
       over, and other paint that lies wholly inside it. A paragraph or cell shading, a border, a list
       number, a leader outside the control or a floating picture stays on
       the page, and so does a floating text box's text that names its own
       paragraph; its fills and its text without a source can still join. Markers move with the page when it is aligned vertically,
       and with realigned tab-aligned text, as link rectangles do.
     - **Why not wrap when drawing.** The paginator reorders elements for
       bidi reflow and tracks them by position. About twenty element passes
       (field substitution, artifacts, source rebinding, font passes) would
       each need a container arm. An empty marker leaves all of them
       unchanged. `FormFieldFragment::mark` (`#[doc(hidden)]`) carries the
       marker until then, and the writer asserts that none reaches it.
     - **What the tests hold.**
       - Replacing every fragment by its children gives the static
         drawing, element for element.
       - Each field's fragments hold exactly its control's text. Labels,
         neighbouring paragraphs, headers, text boxes and shading stay on
         the page.
       - Without the switch, the corpus renders byte-identical.
     - **Values** come from the accepted-view projection the page draws:
       tracked insertions in, deletions out, and a line feed inside a text
       node reads as the space Word shows. A control inside a tracked
       insertion or move is a field like any other, in document order
       within its paragraph, inside a control too (its content and its
       tracked changes by run boundary, as the page draws them). One inside a deletion has no field and never makes the
       control around it a container; it says a tracked change deletes it,
       unless a block field around it covers its paragraph.
     - **Multiline** when the widget spans several lines. An inline control
       that wraps beside other text says that text sits under its box.
     - **Static, with a diagnostic:**
       - a control split across pages or columns, a column told by the
         left edge each marker records;
       - a field that gathered nothing, unless it is a signature field,
         empty by design over a picture that stays on the page or over
         nothing;
       - one in a rotated cell or a vertical section, sections read item
         by item as layout reads them, and a block field with any
         paragraph in a vertical section as a whole;
       - one holding a floating picture;
       - one whose properties do not decode;
       - one that draws nothing, since empty controls have no anchor yet.
     - **Containers and wrappers.** A block field may hold block-level
       markers (bookmarks) and unmapped wrappers. Block controls nested in
       a cell-level control are walked as table layout walks them.
     - **Figures.** Pictures tagged as figures stay on the page under the
       widget, so their structure element keeps its content.
     - Legacy form fields move to their own pull request 5b.
5b. **`feat(docx): legacy form fields as fillable pdf fields`.**
   - Scope: `FORMTEXT`, `FORMCHECKBOX` and `FORMDROPDOWN` marked by their
     field source and numbered with the content controls.
   - Check: the Word layout tests on Word-shaped legacy fields.
   - As built:
     - Lands after pull request 6, so the feature ships end to end (Rust,
       CLI, Python) first. 5b widens what it covers, and narrows the
       limitation pull request 6 adds.
     - A legacy field's runs carry a mark of their own, the field's index
       among the paragraph's fields, including the stored result runs drawn
       as ordinary runs. Fields are numbered with the content controls by
       the first physical run each draws.
     - A content control around a legacy field is a container, as it is
       around another control: the legacy field becomes the field.
     - Legacy marks name the main-story paragraph, as control marks do, and
       a legacy field in a rotated cell or a vertical section stays static
       and says so.
     - A legacy drop-down with no stored result, as Word saves one, draws
       its selected entry (#170) and becomes a field like the others.
     - Word's empty text field, five en spaces, has no value.
     - A legacy field nested in another field, or whose form data does not
       read, stays static with a diagnostic. A field the model does not read
       as one, such as LibreOffice's with a bookmark inside (#139), is not
       seen at all.
6. **`feat(cli,python): fillable pdf forms`.**
   - Scope:
     - the CLI flag, the Python keyword and stubs, the Python tests;
     - the veraPDF job;
     - limitation entries with their issues;
     - the user docs, signature convention included.
   - Check: the Python and CLI tests, the veraPDF job, and a pdftoppm render
     of the fixture attached to the pull request.

## Decisions taken

The orchestration session decided the open questions on 2026-11-21:

1. **Signature marker: the tag convention.** The intent lives in the model,
   and it is set from Word's properties dialog. Word signature lines
   (`o:signatureline`) may come later, on top. An explicit list of tags in
   the options was rejected: the caller would have to know the tags.
2. **Unfilled control: nothing shown.** No value, an empty appearance, the
   placeholder text as the tooltip. Keeping the grey placeholder was
   rejected: it prints, and viewers drop it inconsistently.
3. **Typing font: Liberation Sans, embedded in every PDF with fields.** One
   behaviour, safe for PDF/A, about 190 KB. The standard Helvetica, not
   embedded, was rejected as a PDF/A risk.
4. **`RenderOptions::fillable_forms`, with `RenderOptions` made
   `#[non_exhaustive]` in the same `feat(docx)!`.** One break now rather
   than one per option. Separate methods were rejected: there is already a
   dozen `to_pdf*` and `save_pdf*` methods.

## Links

- [ROADMAP.md](../ROADMAP.md), item 4.
- [rdocx#381](https://github.com/tensorbee/rdocx/issues/381), the original
  request and acceptance.
- [2026-11-21-corpus-at-scale-and-behaviour-tests.md](2026-11-21-corpus-at-scale-and-behaviour-tests.md),
  the `form` family.
