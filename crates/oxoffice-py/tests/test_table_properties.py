"""Table, row and cell property edits on the Word and Google Docs files of the corpus: every handle taken before the
edits still reads its own element and the new values, which a save writes; a refused edit leaves the document as it
was; tables in a cell or a header take the same edits."""

import io
import zipfile
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
    hold an empty w:tcPr: the grid widths give them their first properties."""
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


def built_table(producer):
    """A table of 4 rows and 3 columns: a new document's own, or the first table of a report."""
    if producer == "new":
        doc = oxdocx.Document()
        doc.add_paragraph("before")
        return doc, doc.add_table(4, 3)
    doc = report(producer)
    return doc, doc.tables[0]


@pytest.mark.parametrize("producer", ["new", *PRODUCERS])
def test_row_and_cell_edits_made_in_turn_keep_every_handle(producer):
    """A table filled row by row, a table or row property edit then cell text, as a generator writes one."""
    doc, table = built_table(producer)
    cells = [table.cell(index, 1) for index in range(1, 4)]
    for index, cell in enumerate(cells, start=1):
        table.indent = Pt(index)
        cell.text = f"value {index}"
        table.rows[index].height = Pt(20 + index)
        cell.text = f"value {index}"
        table.cell(index, 2).shading = "EEEEEE"
        cell.vertical_alignment = WD_CELL_VERTICAL_ALIGNMENT.CENTER
    saved = oxdocx.Document.from_bytes(doc.to_bytes())
    for current in (table, saved.tables[0]):
        assert current.indent == Pt(3)
        for index in range(1, 4):
            assert current.rows[index].height == Pt(20 + index)
            assert current.cell(index, 1).text == f"value {index}"
            assert current.cell(index, 1).vertical_alignment == WD_CELL_VERTICAL_ALIGNMENT.CENTER
            assert current.cell(index, 2).shading == RGBColor(0xEE, 0xEE, 0xEE)
    assert [cell.text for cell in cells] == ["value 1", "value 2", "value 3"]


def bare_cells(first_cell="<w:p><w:r><w:t>bare</w:t></w:r></w:p>"):
    """A table of two cells without w:tcPr, the first holding `first_cell`, between two paragraphs."""
    return (
        "<w:p><w:r><w:t>before</w:t></w:r></w:p>"
        '<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w="2000"/><w:gridCol w:w="2000"/></w:tblGrid>'
        f"<w:tr><w:tc>{first_cell}</w:tc><w:tc><w:p><w:r><w:t>neighbour</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"
        '<w:p><w:bookmarkStart w:id="0" w:name="after"/><w:r><w:t>after</w:t></w:r><w:bookmarkEnd w:id="0"/></w:p>'
    )


def document_with_body(body):
    """A new document whose body holds `body` before its section properties."""
    out = io.BytesIO()
    with zipfile.ZipFile(io.BytesIO(oxdocx.Document().to_bytes())) as source, \
            zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as target:
        for item in source.infolist():
            part = source.read(item.filename)
            if item.filename == "word/document.xml":
                xml = part.decode("utf-8")
                start = xml.index("<w:body>") + len("<w:body>")
                part = (xml[:start] + body + xml[xml.index("<w:sectPr"):]).encode("utf-8")
            target.writestr(item, part)
    return oxdocx.Document.from_bytes(out.getvalue())


def saved_cells(doc):
    """The w:tc elements of the saved body, as written."""
    with zipfile.ZipFile(io.BytesIO(doc.to_bytes())) as package:
        body = package.read("word/document.xml").decode("utf-8")
    return [cell.split("</w:tc>")[0] for cell in body.split("<w:tc>")[1:]]


FIRST_CELL_PROPERTIES = {
    "shading": (lambda cell: setattr(cell, "shading", "FFFF00"), lambda cell: cell.shading,
                RGBColor(0xFF, 0xFF, 0x00)),
    "vertical alignment": (lambda cell: setattr(cell, "vertical_alignment", WD_CELL_VERTICAL_ALIGNMENT.CENTER),
                           lambda cell: cell.vertical_alignment, WD_CELL_VERTICAL_ALIGNMENT.CENTER),
    "width": (lambda cell: setattr(cell, "width", Pt(90)), lambda cell: cell.width, Pt(90)),
    "border": (lambda cell: cell.set_border("bottom", "dashed", size=4, color="0000FF"),
               lambda cell: cell.border("bottom"), ("dashed", 4, RGBColor(0x00, 0x00, 0xFF))),
    "margins": (lambda cell: cell.set_margins(top=Pt(5), right=Pt(6), bottom=Pt(7), left=Pt(8)),
                lambda cell: cell.margins, (Pt(5), Pt(6), Pt(7), Pt(8))),
}


@pytest.mark.parametrize("name", FIRST_CELL_PROPERTIES)
def test_a_first_property_of_a_cell_without_properties_reads_back_through_the_handle_taken_before(name):
    """The first property writes the cell's w:tcPr before its paragraphs, which moves them."""
    set_property, read, expected = FIRST_CELL_PROPERTIES[name]
    doc = document_with_body(bare_cells())
    table = doc.tables[0]
    cell, neighbour = table.cell(0, 0), table.cell(0, 1)
    paragraph = cell.paragraphs[0]
    run = paragraph.runs[0]
    after = doc.paragraphs[1]
    assert read(cell) is None
    assert after.text == "after"

    set_property(cell)

    assert read(cell) == expected
    assert read(neighbour) is None
    assert (paragraph.text, run.text, neighbour.text, after.text) == ("bare", "bare", "neighbour", "after")
    # Handles taken before the edit still edit their own element.
    run.bold = True
    after.runs[0].italic = True
    saved = oxdocx.Document.from_bytes(doc.to_bytes())
    table = saved.tables[0]
    assert read(table.cell(0, 0)) == expected
    assert read(table.cell(0, 1)) is None
    assert table.cell(0, 0).paragraphs[0].text == "bare"
    assert table.cell(0, 0).paragraphs[0].runs[0].bold is True
    assert [p.text for p in saved.paragraphs] == ["before", "after"]
    assert saved.paragraphs[1].runs[0].italic is True
    # The properties come first in the cell, as the schema orders them; the neighbour gets none.
    first, second = saved_cells(doc)
    assert first.startswith("<w:tcPr>"), first
    assert "<w:tcPr" not in second, second


def test_grid_widths_give_cells_without_properties_a_width_the_handles_taken_before_read():
    """A table edit that writes the first w:tcPr of its cells cannot go in place either: it is published from a
    candidate document whose handles are relocated."""
    doc = document_with_body(bare_cells())
    table = doc.tables[0]
    cell, neighbour = table.cell(0, 0), table.cell(0, 1)
    paragraph = cell.paragraphs[0]
    after = doc.paragraphs[1]
    assert (cell.width, neighbour.width) == (None, None)

    table.grid_widths = [Pt(100), Pt(80)]

    assert (cell.width, neighbour.width) == (Pt(100), Pt(80))
    assert (paragraph.text, neighbour.text, after.text) == ("bare", "neighbour", "after")
    paragraph.runs[0].bold = True
    saved = oxdocx.Document.from_bytes(doc.to_bytes())
    table = saved.tables[0]
    assert table.grid_widths == (Pt(100), Pt(80))
    assert (table.cell(0, 0).width, table.cell(0, 1).width) == (Pt(100), Pt(80))
    assert table.cell(0, 0).paragraphs[0].runs[0].bold is True
    assert [p.text for p in saved.paragraphs] == ["before", "after"]
    first, second = saved_cells(doc)
    assert first.startswith("<w:tcPr>") and second.startswith("<w:tcPr>"), (first, second)


CELL_CONTENT_OFF_THE_LOCAL_ROUTE = {
    "hyperlink": ('<w:p><w:r><w:t>bare</w:t></w:r><w:hyperlink w:anchor="after"><w:r><w:t> link</w:t></w:r>'
                  "</w:hyperlink></w:p>", "bare link"),
    "nested table": ('<w:p><w:r><w:t>bare</w:t></w:r></w:p><w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w="1000"/>'
                     "</w:tblGrid><w:tr><w:tc><w:p><w:r><w:t>inner</w:t></w:r></w:p></w:tc></w:tr></w:tbl><w:p/>",
                     "bare"),
}
# The handle of what the cell holds, and how it reads.
HELD = {
    "hyperlink": (lambda doc, cell: doc.hyperlinks[0], lambda link: link.text, " link"),
    "nested table": (lambda doc, cell: cell.tables[0], lambda table: table.cell(0, 0).text, "inner"),
}


@pytest.mark.parametrize("content", CELL_CONTENT_OFF_THE_LOCAL_ROUTE)
def test_a_first_property_of_a_cell_holding_a_hyperlink_or_a_table_keeps_the_handles_taken_before(content):
    first_cell, text = CELL_CONTENT_OFF_THE_LOCAL_ROUTE[content]
    doc = document_with_body(bare_cells(first_cell))
    table = doc.tables[0]
    cell = table.cell(0, 0)
    paragraph = cell.paragraphs[0]
    after = doc.paragraphs[1]
    take, read_held, held_text = HELD[content]
    held = take(doc, cell)
    assert (cell.shading, paragraph.text, read_held(held)) == (None, text, held_text)

    try:
        cell.shading = "FFFF00"
    except oxdocx.StaleElementError as error:
        pytest.fail(f"the edit itself was refused: {error}")
    # The saved value first, through the document only: only the old handles may go stale.
    saved = oxdocx.Document.from_bytes(doc.to_bytes()).tables[0]
    assert (saved.cell(0, 0).shading, saved.cell(0, 0).paragraphs[0].text) == (RGBColor(0xFF, 0xFF, 0x00), text)

    assert cell.shading == RGBColor(0xFF, 0xFF, 0x00)
    assert (paragraph.text, after.text, read_held(held)) == (text, "after", held_text)
    assert table.cell(0, 1).shading is None


@pytest.mark.parametrize("content", CELL_CONTENT_OFF_THE_LOCAL_ROUTE)
def test_grid_widths_on_cells_holding_a_hyperlink_or_a_table_keep_the_handles_taken_before(content):
    """The first w:tcPr of a cell shifts what the cell holds, a nested table or a hyperlink's paragraph too."""
    first_cell, text = CELL_CONTENT_OFF_THE_LOCAL_ROUTE[content]
    doc = document_with_body(bare_cells(first_cell))
    table = doc.tables[0]
    cell = table.cell(0, 0)
    paragraph = cell.paragraphs[0]
    after = doc.paragraphs[1]
    take, read_held, held_text = HELD[content]
    held = take(doc, cell)

    table.grid_widths = [Pt(100), Pt(80)]

    assert (cell.width, table.cell(0, 1).width) == (Pt(100), Pt(80))
    assert (paragraph.text, after.text, read_held(held)) == (text, "after", held_text)
    paragraph.runs[0].bold = True
    saved = oxdocx.Document.from_bytes(doc.to_bytes()).tables[0]
    assert saved.cell(0, 0).width == Pt(100)
    assert saved.cell(0, 0).paragraphs[0].runs[0].bold is True


def test_a_first_property_of_a_cell_holding_two_tables_keeps_each_table_handle_on_its_own_table():
    """Owners in a cell story are matched by depth, then in document order: each handle keeps its own table."""
    inner = ('<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w="1000"/></w:tblGrid><w:tr><w:tc><w:p><w:r><w:t>{}</w:t>'
             "</w:r></w:p></w:tc></w:tr></w:tbl>")
    doc = document_with_body(bare_cells("<w:p/>" + inner.format("first") + "<w:p/>" + inner.format("second")
                                        + "<w:p/>"))
    cell = doc.tables[0].cell(0, 0)
    first, second = cell.tables
    assert (first.cell(0, 0).text, second.cell(0, 0).text) == ("first", "second")

    cell.shading = "FFFF00"

    assert (first.cell(0, 0).text, second.cell(0, 0).text) == ("first", "second")
    first.cell(0, 0).paragraphs[0].runs[0].bold = True
    second.cell(0, 0).text = "changed"
    saved = oxdocx.Document.from_bytes(doc.to_bytes()).tables[0].cell(0, 0)
    assert saved.shading == RGBColor(0xFF, 0xFF, 0x00)
    assert [table.cell(0, 0).text for table in saved.tables] == ["first", "changed"]
    assert saved.tables[0].cell(0, 0).paragraphs[0].runs[0].bold is True

