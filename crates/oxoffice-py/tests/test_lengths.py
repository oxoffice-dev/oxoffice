"""Every Python setter that takes a length reads a bare 0 as zero, like Pt(0), Twips(0) and Cm(0): scripts carried
from python-docx write `space_before = 0` for "no spacing". Read back after a save."""
import pytest

import oxoffice.docx as docx
from oxoffice.docx import Cm, Pt, Twips

ZEROS = [0, Pt(0), Twips(0), Cm(0)]


def reopened(doc):
    return docx.Document.from_bytes(doc.to_bytes())


@pytest.mark.parametrize("zero", ZEROS, ids=["bare", "Pt", "Twips", "Cm"])
@pytest.mark.parametrize("name", ["space_before", "space_after", "first_line_indent", "left_indent", "right_indent"])
def test_a_paragraph_length_takes_a_zero_in_every_form(name, zero):
    doc = docx.Document()
    doc.add_paragraph("text")
    setattr(doc.paragraphs[0].paragraph_format, name, Pt(6))
    setattr(doc.paragraphs[0].paragraph_format, name, zero)
    assert getattr(reopened(doc).paragraphs[0].paragraph_format, name) == 0


@pytest.mark.parametrize("zero", ZEROS, ids=["bare", "Pt", "Twips", "Cm"])
def test_cell_margins_take_a_zero_in_every_form(zero):
    doc = docx.Document()
    table = doc.add_table(rows=1, cols=1)
    table.set_cell_margins(top=Pt(4), right=Pt(4), bottom=Pt(4), left=Pt(4))
    table.set_cell_margins(top=zero, right=zero, bottom=zero, left=zero)
    assert reopened(doc).tables[0].cell_margins == (0, 0, 0, 0)


# Known failure #343: the document defaults refuse a bare 0 ("requires a Length or None"). When it passes, drop the
# xfail.
@pytest.mark.xfail(strict=True, raises=TypeError, reason="#343")
def test_the_default_paragraph_spacing_takes_a_bare_zero():
    doc = docx.Document()
    doc.set_default_paragraph_format(space_before=Pt(6), space_after=Pt(6))
    try:
        doc.set_default_paragraph_format(space_before=0, space_after=0)
    except TypeError as refusal:
        assert "requires a Length" in str(refusal), refusal  # the #343 refusal, nothing else
        raise
    defaults = reopened(doc).default_paragraph_format
    assert (defaults.space_before, defaults.space_after) == (0, 0)
