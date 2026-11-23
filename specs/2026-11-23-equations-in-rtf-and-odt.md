---
date: 2026-11-23
status: accepted
scope: docx
---

# Equations in the RTF and ODT exports

## Context

`to_rtf_bytes` and `to_odt_bytes` leave every OfficeMath equation out (issue #380). Each one is reported (`Word
equation was dropped during RTF export`), but the text around it closes up: `Let x=1 hold.` reads `Let  hold.`.
`word/paper.docx` of the corpus has 11 equations. The HTML, Markdown, EPUB and MHTML exports write them since #262,
through `equation_to_mathml` and `equation_to_latex` (`ExportEquations` in `math.rs`); the PDF draws them (#224).

Both formats have a native carrier:

- **RTF** has OMML-in-RTF: `{\mmath{\*\moMath ...}}`, each OMML element `m:X` spelled as a group `{\mX ...}`, a
  property's `m:val` as the group's text (`{\mbegChr (}`), the text of a math run in `{\mr ...}`. LibreOffice
  writes it from `word/paper.docx` (`{\mmath {\*\moMath {\msSub {\me {\mr y}}{\msub {\mr i}...}}}`) and reads it
  back (its reader maps the same names back to OMML elements, `rtftokenizer.cxx`). Word writes and reads it. A
  reader that does not know `\mmath` reads its text, the equation's letters in order, which is the fallback the
  format chose.
- **ODT** has a formula object: `<draw:frame><draw:object xlink:href="./Object 1"/></draw:frame>` in the text, and a
  sub-document `Object 1/content.xml` whose root is a MathML `<math>`, listed in the manifest as
  `application/vnd.oasis.opendocument.formula`. LibreOffice writes and reads it.

## Decision

**RTF and ODT write every equation in its place, in the format's own math, and their readers read it back.**

- **RTF writer**: an equation is written from its OMML (`OfficeMath::to_xml`, so the content outside the typed
  subset is kept too) by one generic walk: an element `m:X` with children is `{\mX children}`; an element whose only
  content is `m:val` is `{\mX value}`; a math run `m:r` is `{\mr flags text}`, its `m:rPr` written as the run's
  flags. A run's alignment point and break (`m:aln`, `m:brk`) have no Word file showing their spelling inside a
  run: they are reported, not written. Its `w:rPr` is not carried: the math font and italic are the reader's own,
  other formatting (bold, colour) is reported. The
  first element is starred (`{\mmath{\*\moMath ...}}`, `{\mmath{\*\moMathPara ...}}` for a display group), as Word
  and LibreOffice write it. The spelling of what the generic rule does not cover (`m:sty`, `m:scr` and the other
  run flags, the `\mmathPr` header) is taken from an RTF that Word saves (Word 16.115 on the Mac,
  `word-paper.rtf`): values are group text with the OMML string (`{\mpos top}`, `{\mdegHide on}`), numbers only
  for `\mjc` (center 2, centerGroup 1) and the run flags `\mscr0` (roman), `\msty0` (p), `\msty2` (i). The other
  `sty`, `scr` and `jc` values have no Word file showing their numbers: they are left out and reported. The reader
  reads `\mscr0` and `\msty2`, which Word writes on every run, as OMML's defaults (no `m:scr`, no `m:sty`), so a
  built equation reads back as the same OMML.
- **No picture fallback**: Word adds `{\mmathPict ...}`, a picture of the equation, for readers that do not know
  math. We do not draw one: the RTF writer does not render.
- **RTF reader**: `\mmath` is read back into the paragraph's equation at its place, by the inverse walk; a group it
  cannot map is reported and left out; an equation it cannot rebuild as OMML is reported and its text kept as body
  text, as a reader without math would show it (before this change the reader kept every equation's letters so).
- **ODT writer**: an equation is a `draw:frame` anchored `as-char` holding a `draw:object` that points at its own
  sub-document `Object N/content.xml`, the MathML that `equation_to_mathml` writes for the HTML export, with the same
  diagnostics (`ExportEquations`), each reported at its equation. The equations of a display group are frames
  separated by line breaks. The frame has no size: LibreOffice sizes a formula object from its content when it
  opens it.
- **ODT reader**: a `draw:object` whose sub-document is a formula is read back through the MathML reader, its root
  `display` attribute accepted (LibreOffice writes `display="block"` on every formula, wherever it stands); an
  object it cannot read is reported and left out. A paragraph that holds only formulas (and line breaks) reads
  as a display group, as Word shows an equation alone on its line; a formula among text is inline.
- **Diagnostics**: `Word equation was dropped during RTF/ODT export` goes; what the conversion loses is reported per
  equation as the HTML export reports it.
- **Surface**: no new call. `to_rtf_bytes`, `to_odt_bytes`, `from_rtf_bytes`, `from_odt_bytes`, their Python
  twins. The CLI has no RTF or ODT conversion (the issue says it does; it does not), so nothing changes there.

**What we do not do**:

- No MathML to OMML for RTF: RTF gets Word's own math, not a conversion, so nothing outside the typed subset is lost.
- No StarMath annotation in the ODT formula (`<annotation encoding="StarMath 5.0">`): LibreOffice rebuilds it from
  the MathML when the formula is edited.
- No picture fallback in either format (above).

## Plan

One pull request, `feat(docx): rtf and odt exports write equations`, closing #380.

1. RTF writer and reader, tested on built equations (fraction, sub/superscript, n-ary, delimiter, radical with a
   hidden degree, a display group, a run with formatting) and on `word/paper.docx`: export, read back, compare the
   OMML; LibreOffice headless opens the RTF and its ODT conversion holds 11 formulas.
2. ODT writer and reader, the same documents: the package validates (manifest lists each object), read back gives
   the same MathML, LibreOffice opens it with 11 formulas.
3. Word check (Mac session): Word opens our RTF of `word/paper.docx` with 11 equations; Word's own RTF of the same
   file fixes the spelling of the run flags and the header.

## Links

- Issue #380; #262 (the web exports); #224 (PDF).
- LibreOffice: `sw/source/writerfilter/rtftok/rtftokenizer.cxx` (math keywords), `starmath/source/rtfexport.cxx`.
