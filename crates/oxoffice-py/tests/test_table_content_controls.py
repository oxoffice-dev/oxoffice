"""A table's rows and cells count those of its row and cell content controls, as Word writes repeating table rows,
in Python as in the Rust facade: counted, read, set, copied and removed by the same numbers. The tag setters and the
listing of content controls reach a control in a header."""

import io
import zipfile

import pytest

import oxoffice.docx as oxdocx


def cell(text):
    return f"<w:tc><w:p><w:r><w:t>{text}</w:t></w:r></w:p></w:tc>"


def row(name):
    return f"<w:tr>{cell(name + '1')}{cell(name + '2')}</w:tr>"


def control(tag, content):
    return f'<w:sdt><w:sdtPr><w:tag w:val="{tag}"/></w:sdtPr><w:sdtContent>{content}</w:sdtContent></w:sdt>'


def grid(columns):
    return "<w:tblGrid>" + '<w:gridCol w:w="1500"/>' * columns + "</w:tblGrid>"


# Rows A, then a row control holding B and a nested row control holding C, then D.
ROWS = f"<w:tbl>{grid(2)}{row('A')}{control('lines', row('B') + control('inner', row('C')))}{row('D')}</w:tbl><w:p/>"
# One row: A, then a cell control holding X and a nested cell control holding Y, then D; a second row E F G H.
CELLS = (
    f"<w:tbl>{grid(4)}<w:tr>{cell('A')}{control('cells', cell('X') + control('inner', cell('Y')))}{cell('D')}</w:tr>"
    f"<w:tr>{cell('E')}{cell('F')}{cell('G')}{cell('H')}</w:tr></w:tbl><w:p/>"
)


def with_body(body):
    """A new document whose body is `body`."""
    source = oxdocx.Document().to_bytes()
    out = io.BytesIO()
    with zipfile.ZipFile(io.BytesIO(source)) as package, zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as target:
        for item in package.infolist():
            part = package.read(item.filename)
            if item.filename == "word/document.xml":
                xml = part.decode("utf-8")
                start = xml.index("<w:body>") + len("<w:body>")
                end = xml.find("<w:sectPr")
                part = (xml[:start] + body + xml[end if end >= 0 else xml.index("</w:body>"):]).encode("utf-8")
            target.writestr(item, part)
    return oxdocx.Document.from_bytes(out.getvalue())


def reopen(doc):
    return oxdocx.Document.from_bytes(doc.to_bytes())


def first_column(doc):
    return [r.cells[0].text for r in doc.tables[0].rows]


def test_rows_of_row_controls_are_counted_read_and_set():
    doc = with_body(ROWS)
    table = doc.tables[0]
    assert len(table.rows) == 4
    assert first_column(doc) == ["A1", "B1", "C1", "D1"]
    assert table.cell(2, 1).text == "C2"
    assert table.rows[1].cells[1].paragraphs[0].text == "B2"
    assert "C1" in table.cell(2, 0).xml

    table.cell(2, 0).text = "Changed"
    table.rows[1].cells[1].text = "Bee"
    table.rows[2].cells[1].paragraphs[0].text = "Sea"
    table.set_cell_vertical_merge(1, 0, "restart")
    doc = reopen(doc)
    table = doc.tables[0]
    assert [[c.text for c in r.cells] for r in table.rows] == [
        ["A1", "A2"], ["B1", "Bee"], ["Changed", "Sea"], ["D1", "D2"]
    ]
    assert table.cell(1, 0).vertical_merge == "restart"
    assert table.cell(3, 0).vertical_merge is None


def test_rows_are_copied_and_removed_by_the_counted_rows():
    doc = with_body(ROWS)
    assert doc.tables[0].clone_row(0, 1).cells[0].text == "A1"
    assert first_column(doc) == ["A1", "A1", "B1", "C1", "D1"]
    with pytest.raises(oxdocx.DocxError, match="row inside a row content control"):
        doc.tables[0].remove_row(3)  # row C sits in a content control
    doc.tables[0].remove_row(4)
    doc = reopen(doc)
    assert first_column(doc) == ["A1", "A1", "B1", "C1"]


def test_cells_of_cell_controls_are_counted_read_and_set():
    doc = with_body(CELLS)
    table = doc.tables[0]
    first = table.rows[0]
    assert len(first.cells) == 4
    assert [c.text for c in first.cells] == ["A", "X", "Y", "D"]
    assert table.cell(0, 2).text == "Y"
    assert table.cell(1, 2).text == "G"

    table.cell(0, 2).text = "Why"
    first.cells[1].paragraphs[0].text = "Ex"
    table.set_cell_vertical_merge(0, 3, "restart")
    doc = reopen(doc)
    table = doc.tables[0]
    assert [c.text for c in table.rows[0].cells] == ["A", "Ex", "Why", "D"]
    assert table.cell(0, 3).vertical_merge == "restart"
    assert table.cell(0, 0).vertical_merge is None


def test_the_tag_setter_and_the_listing_reach_a_control_in_a_header():
    doc = oxdocx.Document()
    doc.add_paragraph("Body")
    header = doc.create_section_story(0, "header", "default")
    doc.insert_content_control(oxdocx.ContentLocation.end(header), "block", "plain_text", tag="title")
    assert [c.tag for c in doc.content_controls()] == ["title"]
    assert doc.set_content_control_value_by_tag("title", "Annual report") == 1
    doc = reopen(doc)
    assert [(c.tag, c.text) for c in doc.content_controls()] == [("title", "Annual report")]
