---
date: 2026-11-23
status: accepted
scope: docx
---

# Equation constructs as tall as Word draws them

## Context

Equation constructs take a different height than Word gives them, so lines holding them move (issue #405). Measured
with Word 16.115 on probe documents (Cambria 11 text, single lines, no space after; the baseline steps before and
after each equation, display then in a line of text):

| Construct | Word display | oxoffice display | Word in text | oxoffice in text |
| --- | --- | --- | --- | --- |
| y = a/b | 17.52 / 18.00 | 21.69 / 19.43 | 14.51 / 16.02 | 21.69 / 19.43 |
| sum, limits under and over | 25.50 / 24.53 | 27.22 / 25.22 | 13.50 / 13.25 | 27.22 / 25.22 |
| integral 0..1, limits beside | 21.78 / 21.00 | 14.49 / 14.14 | 15.75 / 15.00 | 14.49 / 14.14 |
| 3x3 matrix in brackets | 25.50 / 25.52 | 36.30 / 25.78 | 25.50 / 25.52 | 36.30 / 25.78 |
| (a/b)/(c/d) | 26.25 / 26.50 | 37.02 / 34.72 | 20.50 / 20.25 | 37.02 / 34.72 |
| e^x | 12.75 / 13.00 | 14.49 / 12.89 | 13.00 / 12.74 | 14.49 / 12.89 |

The cause is one: the bundled math face, Caladea (Cambria's metric twin), has no OpenType MATH table, and the layout
falls back to em constants written for no font in particular. Every construct takes them, and a line of text is set
like a display equation.

Word's glyph positions (its PDFs, read on the Mac) follow the OpenType MATH model with Cambria Math's constants, to
the quarter point Word rounds to:

- **The frame**: an equation's line box starts `MathLeading` above its ink and ends at the ink's bottom, never
  inside a line of the math font (E2 display: numerator ink 13.48 + 1.61 = 15.09 above the baseline, Word 15.08).
- **A line of text** sets its equation in the text style: fraction parts and n-ary limits one script size down
  (`ScriptPercentScaleDown`, 8 pt for 11), n-ary limits beside the text-size sign. A display equation keeps its
  nested fractions at full size, with the text-style shifts.
- **n-ary signs** are centred on the math axis (`AxisHeight`). A display sum takes Cambria Math's first variant at
  least `DisplayOperatorMinHeight` tall; a display integral the next one. An integral, or any display-size sign,
  places its scripts by its ink.
- **Matrices** are centred on the math axis by their ink; **delimiters** are centred on the axis and grow to cover
  their contents on both sides of it once those are taller than `DelimitedSubFormulaMinHeight`.

## Decision

**A face without a MATH table is laid out with Cambria Math's constants**, so the fallback path is the MATH path,
and the construct rules above apply to every face.

- `MathConstants` holds the constants the layout reads: the face's MATH table, else Cambria Math's (version 6.90i,
  as Word 16.115 ships it, read with fonttools on the Mac; numbers only, no font file).
- For the display sum and integral without MATH variants, the sign's glyph is stretched to the ink of Cambria Math's
  display sign (sum -1370..2540, integral -1623..2782 in its 2048 units).
- The radical without variants stretches its own glyph the same way, with the MATH constants for gap and rule.
- Not done: horizontal metrics (widths, gaps between atoms) are not measured against Word. They move where the MATH
  rules set them: `SpaceAfterScript` after scripts, a bar fraction without the 0.06 em side padding (its rule flush
  with its parts), a radical without its 0.9 horizontal squeeze and with its degree placed by
  `RadicalKernBeforeDegree` and `RadicalKernAfterDegree`, and signs that change size. A face with a MATH table keeps
  choosing its display sign as the first variant at least `DisplayOperatorMinHeight` tall.
- A linear or skewed fraction keeps its parts at its slash's size in a line of text too.

## Plan

One pull request, closes #405. Checked by one test per construct in `crates/oxoffice-docx/tests/docx/pdf.rs`
(`MathProbe`: Word's probe built the same way, its four baseline steps within a point of Word's), each failing on
main with the numbers of the table above; the render baselines of the corpus files with equations rewritten in a
commit of their own, and their page counts checked against Word's.

## Links

- Issue #405; #224 and [every OfficeMath construct drawn](2026-11-23-every-officemath-construct-drawn.md); #411.
- Word's probes: the generator and Word's baselines are in #405's details.
