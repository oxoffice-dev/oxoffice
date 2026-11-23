---
date: 2026-11-23
status: accepted
scope: docx
---

# Every OfficeMath construct is drawn

## Context

Since #411 the layout draws equations, but every equation of the corpus paper and thesis, in each producer's copy,
still reports `OfficeMath content at ... was preserved but could not be rendered` (13 in a paper, 6 in a thesis),
and the render tests list the 8 files as known failures of `dropped` (issue #224). Two different things hide behind
that one message:

- **Formatting the model keeps raw.** Word writes a `w:rPr` (the run's font, size, colour, bold) in every `m:r` and
  an `m:ctrlPr` (the formatting of the construct's own characters) in every construct's properties. The reader
  keeps both verbatim as raw children, and `has_unsupported_content` counts any raw child as content left out. The
  equation draws, but the check says it did not.
- **Constructs the model does not type.** ECMA-376 Part 1, 22.1.2 has 22 OfficeMath objects; the model types 13.
  The others are kept raw and not drawn: `m:func` (a function apply, "log x": the paper's "log" is missing),
  `m:bar` (a bar over or under its base: the paper's x-bar), `m:box` (Google Docs wraps an operator in one),
  an `m:nary` without its `m:e` (Google Docs writes its sums so, the summand after them),
  `m:borderBox`, `m:groupChr` (a brace under or over a group), `m:eqArr` (aligned equations), `m:phant` (a
  phantom). The matrix properties `m:mcs` (column justification), `m:plcHide`, `m:rSpRule`, `m:cGpRule` and
  `m:cGp` are kept raw the same way.

And `∼` (U+223C) and `⊤` (U+22A4) are drawn as empty boxes with the bundled fonts: none of them has the Mathematical
Operators block whole, and the default math face, Caladea (Cambria's metric twin), has no MATH table.

## Decision

**The model types every OfficeMath object of 22.1.2, the layout draws each of them, and the diagnostic is left to
content no reader understands.**

- **Model** (`oxoffice-docx-oxml/src/math.rs`): new `MathExpression` variants `Function` (`m:func`: name, argument),
  `Bar` (`m:bar`: position top or bottom, base), `Box` (`m:box`: base and its `opEmu`, `noBreak`, `diff`, `aln`
  flags), `BorderBox` (`m:borderBox`: base and the hide and strike flags), `GroupCharacter` (`m:groupChr`: character,
  position, vertical justification, base), `EquationArray` (`m:eqArr`: rows, base justification, row spacing) and
  `Phantom` (`m:phant`: show, zero width/ascent/descent, transparent, base), with `MathVerticalPosition` for
  `ST_TopBot`. `MathMatrixProperties` gains `plcHide`, `rSpRule`, `cGpRule` and `cGp`; the column properties
  `m:mcs` stay as read and are understood, read through `MathMatrix::column_justification`. An `m:nary` may have no
  `m:e` (Google Docs) and is written back without one. Each construct writes back what it read, unknown attributes
  and children where they were (rule 7); a property is written in its canonical form, as the other constructs do
  (an on/off with no value gets `m:val="1"`). `MathExpression::arguments` and `arguments_mut` list any construct's
  arguments, so a walk over the tree needs no arm per construct.
- **Run formatting is understood.** The `w:rPr` of an `m:r` (one, tracking no insertion, deletion or move) and an
  `m:ctrlPr` holding only such a `w:rPr` are no longer unsupported content; `MathRun::word_properties` reads the
  first. The layout draws a math run in its colour (`w:color`, a theme colour, a text fill) and its size (`w:sz`,
  scaled by the run's script level: a 16 pt superscript is to a plain one what 16 pt is to the equation's size).
  When every run of an equation sets one size or one colour, as Word writes a whole equation set to 16 pt or red,
  the equation takes it: its rules, signs and delimiters too. Each equation of a display group (`m:oMathPara`)
  is read on its own.
  Bold and italic stay `m:sty`'s, the font stays the math font. `m:ctrlPr` styles the construct's own characters (a
  delimiter, a bar, a fraction rule): the layout draws them with the equation's formatting, as before. A `w:rPr`
  or `m:ctrlPr` holding anything else (a tracked change, an attribute) is still reported.
- **`MathExpression` becomes `#[non_exhaustive]`**, so that the next construct is not a breaking change. The facade
  re-exports it: the commit is marked `!`.
- **Layout** (`oxoffice-docx-layout/src/math.rs`), following Word's drawing:
  - `Function`: name upright, a thin space, then the argument.
  - `Bar`: a rule above or below the base, the gap and thickness of an accent bar. Without a position, or with
    an `m:pos` that has no value as Google Docs writes it, the bar is below: Word 16.115 draws Google's
    `<m:pos/>` below its base and writes no `m:pos` for a bar below (checked on the Mac,
    `handoff/pcwe/word-checks/eqnm-bar-pos.md`).
  - `Box`: the base as is (the flags change line breaking and alignment only).
  - `BorderBox`: a rectangle around the base, sides hidden as the flags say, strikes drawn across.
  - `GroupCharacter`: the character (default U+23DF, bottom curly bracket) stretched to the base's width, below
    or above it.
  - `EquationArray`: rows stacked with Word's row spacing rules, aligned on their first `&` (not drawn), a row
    with no mark starting at that column; with no mark in any row, the rows are centred (`gathered` in LaTeX).
    The array is centred on the math axis.
  - `Phantom`: the base's box, ink drawn only when `show` is on, each dimension zeroed when asked; the ink keeps
    its baseline (a shown phantom with no ascent is Word's smash).
  - Matrix columns: left, centre or right per `m:mcs`.
- **Glyphs**: a math fallback face is bundled, a subset of Noto Sans Math (SIL OFL 1.1, as the other Noto faces)
  limited to the Mathematical Operators, Supplemental Mathematical Operators, Miscellaneous Technical, Arrows,
  Letterlike Symbols and Miscellaneous Mathematical Symbols A and B blocks, without its MATH and layout tables:
  995 characters, 104 KB (`fonts/SUBSET-NotoSansMath.md`). The bundled fallback takes it only for characters the
  text faces lack. It is the last bundled face, so that adding it moves no other face; the PDF font resource
  names still move (no pixel does) in the documents whose text needs a fallback face.
- **Exports**: MathML and LaTeX write the new constructs (a function apply `U+2061`, `\overline` / `\underline`,
  `menclose` / `\boxed`, `\underbrace` / `\overbrace`, `mphantom` / `\phantom`, `mtable` / `aligned`). What
  they cannot carry (a border box's hidden sides, a phantom's zeroed dimensions, a group character LaTeX has no
  command for) is reported. A math run's own colour and size (`w:color`, `w:sz`), which the layout draws since
  step 1, reach MathML as an `mstyle` (`mathcolor`, `mathsize`), so the HTML and EPUB exports keep them; LaTeX,
  and so the Markdown export, reports them as left out (the ODT export writes no equation today), and two runs
  that differ in them are no longer joined. Their readers keep
  producing what they produce today, and the existing constructs convert as before.

**What we do not do**:

- No line breaking inside an equation: `m:box`'s `noBreak` and the `m:brk` of a run change nothing drawn today.
- `m:ctrlPr` formatting that differs from the equation's (a red delimiter in a black equation) is kept and
  written back, not drawn; an `m:ctrlPr` is understood in a construct's properties and at the end of an argument.
- The other properties of a math run's `w:rPr` (underline, highlight, `w:b`, `w:rFonts`) are kept and written
  back, not drawn.
- Word's sizes of each construct are issue #405, a separate pull request on the same files. The matrix spacing
  rules (`rSpRule`, `cGpRule`, `cGp`) are typed and written back, but the matrix keeps today's spacing until #405
  sets its geometry.

## Plan

1. `fix(docx): an equation's run formatting is read, not reported as left out`: `w:rPr` and `m:ctrlPr`
   understood, colour and size drawn, the math fallback face. The Word, LibreOffice and authored theses pass
   `dropped`, the four papers `missing-glyph`; the papers and the Google thesis keep one diagnostic each. Tests: an
   equation with Word's `w:rPr` and `m:ctrlPr` raises no diagnostic and `∼⊤` draw glyphs; a run's colour and size
   are drawn, in a script too; a `w:rPr` tracking an insertion is still reported.
2. `feat(docx)!: every OfficeMath construct is typed and drawn`: the seven constructs, the matrix properties and the
   base-less n-ary in the model, the layout, MathML and LaTeX. Closes #224: removes its last entries of
   `tests/render/known.toml`, the baselines rewritten in a commit of their own. Tests: each construct round-trips
   its XML, draws its ink where Word does, and converts to MathML and LaTeX.

## Links

- Issue #224 (equations left out of PDF and images), #405 (construct sizes), #411 (display equation lines).
- [Render fidelity](2026-11-21-render-fidelity.md).
