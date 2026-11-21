---
date: 2026-11-21
status: accepted
scope: python
---

# One rule for every Python length argument

## Context

The Python bindings read a length argument in four ways (#343):

- `ParagraphFormat.space_before`, the paragraph indents, the table, row, cell, tab and style lengths, the section
  margins and `ListLevel` took any integer and silently stored a bare 1-634 EMU as 0 twips (a bare 1-6349 as 0
  half-points for Word font sizes). `space_before = 12`, meant as points, lost the spacing without a word.
- `space_after` and `Font.character_spacing` refused that sub-twip bare integer.
- `Document.update_section` and `set_default_paragraph_format` refused every bare integer but 0.
- `bool` was read as 1 or 0 EMU by the typed integer arguments, and refused by the others, in both packages.

## Decision

Option A of the [proposal](https://github.com/oxoffice-dev/oxoffice/issues/343#issuecomment-7062744627), chosen by the
user ([decision](https://github.com/oxoffice-dev/oxoffice/issues/343#issuecomment-7062757577)). One reader in
`oxoffice-py-support` takes every EMU length argument of `oxoffice.docx` and `oxoffice.pptx`:

- `None` clears where the property clears, and keeps the default where it did;
- a `Length` of the package is taken as given, even when it rounds to zero;
- a bare `0` is accepted;
- a bare nonzero `int`, or an int-like object with `__index__`, is EMU, refused with `ValueError` when it would round
  to zero in the storage unit: twips (635 EMU), Word half-points (6350 EMU), PowerPoint hundredths of a point
  (127 EMU); EMU-stored drawings and PowerPoint geometry do not round;
- `bool`, `float` and any other type are refused with `TypeError`.

One message per case, naming the argument. The range and sign checks stay.

Not touched, because a bare number there is not EMU: `line_spacing` (a line multiple), pptx `bullet_size` (a
fraction), a float pptx `space_before` or `space_after` (a line multiple), `TextFrame.fit_text(max_size)` (points),
border sizes and `set_page_border(width, space)`, `checkbox(size_half_points)`.

Not chosen: option B, `Length` or a bare 0 only, which breaks `Length` arithmetic (`Inches(1) + Inches(0.5)` is an
`int`).

## Plan

One pull request, `fix(python)!`, with a `BREAKING CHANGE` footer. It is checked by
`crates/oxoffice-py/tests/test_length_arguments.py`: one row per argument, each checked with zeros, a `Length`, a
bare 12 and the largest bare integers that round to zero (`unit - 1` and its negative), a bare unit, a `Length` that
rounds to zero, `True`, `False`, `1.5` and `None`.

## Links

- Issue #343, and the issue comments above.
