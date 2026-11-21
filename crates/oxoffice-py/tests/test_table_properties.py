"""Table, row and cell property edits on the Word and Google Docs files of the corpus: every handle taken before the
edits still reads its own element and the new values, which a save writes; a refused edit leaves the document as it
was; tables in a cell or a header take the same edits."""

from pathlib import Path

import pytest

import oxoffice.docx as oxdocx
from oxoffice.docx.enum.table import WD_CELL_VERTICAL_ALIGNMENT, WD_ROW_HEIGHT_RULE, WD_TABLE_ALIGNMENT
from oxoffice.docx.shared import Pt, RGBColor

CORPUS = Path(__file__).resolve().parents[3] / "tests" / "corpus" / "files"
PRODUCERS = ["word", "google"]


def report(producer):
    return oxdocx.Document.from_bytes((CORPUS / producer / "report.docx").read_bytes())


def edit_properties(table, row, cell):
    """One edit of each kind of table, row and cell property, through the handles given. On the Google file the cells
    have no properties: the grid widths give them their first ones."""
    table.alignment = WD_TABLE_ALIGNMENT.RIGHT
    table.indent = Pt(6)
    table.autofit = False
    table.first_row = False
    table.banded_rows = False
    table.set_borders("single", size=8, color="FF0000")
    table.set_border("top", "double", size=12, color="00FF00")
    table.set_cell_margins(top=Pt(1), right=Pt(2), bottom=Pt(3), left=Pt(4))
    table.grid_widths = [Pt(100), Pt(80), Pt(80), Pt(80)]
    table.set_column_width(1, Pt(90))
    table.set_cell_vertical_merge(2, 3, "restart")
    table.set_cell_vertical_merge(3, 3, "continue")
    row.height = Pt(30)
    row.height_rule = WD_ROW_HEIGHT_RULE.EXACTLY
    row.cant_split = True
    row.is_header = True
    cell.vertical_alignment = WD_CELL_VERTICAL_ALIGNMENT.CENTER
    cell.shading = "FFFF00"
    cell.set_border("bottom", "dashed", size=4, color="0000FF")
    cell.set_margins(top=Pt(5), right=Pt(6), bottom=Pt(7), left=Pt(8))


def properties(table, row, cell):
    return {
        "alignment": table.alignment, "indent": table.indent, "autofit": table.autofit,
        "first_row": table.first_row, "banded_rows": table.banded_rows, "top": table.border("top"),
        "bottom": table.border("bottom"), "cell_margins": table.cell_margins, "grid": table.grid_widths,
        "merge": (table.cell(2, 3).vertical_merge, table.cell(3, 3).vertical_merge), "height": row.height,
        "height_rule": row.height_rule, "cant_split": row.cant_split, "is_header": row.is_header,
        "vertical_alignment": cell.vertical_alignment, "shading": cell.shading, "cell_bottom": cell.border("bottom"),
        "margins": cell.margins, "width": cell.width,
    }


EXPECTED = {
    "alignment": WD_TABLE_ALIGNMENT.RIGHT, "indent": Pt(6), "autofit": False, "first_row": False,
    "banded_rows": False, "top": ("double", 12, RGBColor(0x00, 0xFF, 0x00)),
    "bottom": ("single", 8, RGBColor(0xFF, 0x00, 0x00)), "cell_margins": (Pt(1), Pt(2), Pt(3), Pt(4)),
    "grid": (Pt(100), Pt(90), Pt(80), Pt(80)), "merge": ("restart", "continue"), "height": Pt(30),
    "height_rule": WD_ROW_HEIGHT_RULE.EXACTLY, "cant_split": True, "is_header": True,
    "vertical_alignment": WD_CELL_VERTICAL_ALIGNMENT.CENTER, "shading": RGBColor(0xFF, 0xFF, 0x00),
    "cell_bottom": ("dashed", 4, RGBColor(0x00, 0x00, 0xFF)), "margins": (Pt(5), Pt(6), Pt(7), Pt(8)),
    "width": Pt(100),
}


@pytest.mark.parametrize("producer", PRODUCERS)
def test_property_edits_keep_every_handle_and_read_what_the_save_writes(producer):
    doc = report(producer)
    table = doc.tables[0]
    row, cell = table.rows[1], table.cell(1, 0)
    paragraph = cell.paragraphs[0]
    run = paragraph.runs[0]
    neighbour = table.cell(2, 1).paragraphs[0]
    other = doc.tables[1].cell(0, 0)
    body = doc.paragraphs[0]
    texts = (paragraph.text, run.text, neighbour.text, other.text, body.text)

    edit_properties(table, row, cell)

    assert properties(table, row, cell) == EXPECTED
    assert (paragraph.text, run.text, neighbour.text, other.text, body.text) == texts
    # Handles taken before the edits still edit their own element.
    run.bold = True
    neighbour.runs[0].italic = True
    other.shading = "00FFFF"
    saved = oxdocx.Document.from_bytes(doc.to_bytes())
    table = saved.tables[0]
    assert properties(table, table.rows[1], table.cell(1, 0)) == EXPECTED
    assert table.cell(1, 0).paragraphs[0].runs[0].bold is True
    assert table.cell(2, 1).paragraphs[0].runs[0].italic is True
    assert saved.tables[1].cell(0, 0).shading == RGBColor(0x00, 0xFF, 0xFF)
    assert table.cell(1, 0).paragraphs[0].text == texts[0]
    assert table.cell(2, 1).paragraphs[0].text == texts[2]


@pytest.mark.parametrize("producer", PRODUCERS)
def test_a_refused_property_edit_leaves_the_document_as_it_was(producer):
    doc = report(producer)
    table = doc.tables[0]
    cell = table.cell(1, 0)
    run = cell.paragraphs[0].runs[0]
    before = doc.to_bytes()
    width = table.grid_widths[0]
    with pytest.raises(oxdocx.DocxError, match="must be positive"):
        table.grid_widths = [width, 0, width, width]
    with pytest.raises(oxdocx.DocxError, match="exactly 4"):
        table.grid_widths = [width]
    with pytest.raises(ValueError, match="nonnegative"):
        table.set_column_width(0, -Pt(1))
    with pytest.raises(ValueError, match="hexadecimal"):
        cell.shading = "not a colour"
    with pytest.raises(oxdocx.DocxError, match="negative"):
        table.rows[1].height = -Pt(1)
    assert doc.to_bytes() == before
    run.bold = True
    assert oxdocx.Document.from_bytes(doc.to_bytes()).tables[0].cell(1, 0).paragraphs[0].runs[0].bold is True


@pytest.mark.parametrize("producer", PRODUCERS)
@pytest.mark.parametrize("owner", ["cell", "header"])
def test_a_table_in_a_cell_or_a_header_takes_property_edits(producer, owner):
    doc = report(producer)
    if owner == "cell":
        doc.tables[0].cell(1, 0).add_table(4, 4)

        def inner(document):
            return document.tables[0].cell(1, 0).tables[0]
    else:
        doc.section(0).header.add_table(4, 4)

        def inner(document):
            return document.section(0).header.tables[-1]
    table = inner(doc)
    table.cell(1, 0).text = "inner"
    edit_properties(table, table.rows[1], table.cell(1, 0))
    assert properties(table, table.rows[1], table.cell(1, 0)) == EXPECTED
    saved = inner(oxdocx.Document.from_bytes(doc.to_bytes()))
    assert properties(saved, saved.rows[1], saved.cell(1, 0)) == EXPECTED
    assert saved.cell(1, 0).text == "inner"
