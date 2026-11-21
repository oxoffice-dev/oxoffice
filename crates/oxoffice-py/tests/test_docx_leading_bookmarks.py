"""docx: a bookmark start written between blocks, as Google Docs writes a heading's `_Toc` bookmark, belongs to the
block after it. Inserting before that block goes before the start, and moving or removing the block takes it."""
import io
import zipfile

import oxoffice.docx as oxdocx

LED_HEADING = (
    '<w:p><w:r><w:t>Before</w:t></w:r></w:p>'
    '<w:bookmarkStart w:colFirst="0" w:colLast="0" w:name="_Toc217000001" w:id="4"/>'
    '<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Introduction</w:t></w:r><w:bookmarkEnd w:id="4"/></w:p>'
    '<w:p><w:r><w:t>After</w:t></w:r></w:p>'
)
# Body items: `Before`, the bookmark start, the heading, `After`.
HEADING = 2


def led_heading():
    out = io.BytesIO()
    with zipfile.ZipFile(io.BytesIO(oxdocx.Document().to_bytes())) as source, zipfile.ZipFile(out, "w") as target:
        for item in source.infolist():
            part = source.read(item.filename)
            if item.filename == "word/document.xml":
                xml = part.decode("utf-8")
                start = xml.index("<w:body>") + len("<w:body>")
                part = (xml[:start] + LED_HEADING + xml[xml.index("<w:sectPr") :]).encode("utf-8")
            target.writestr(item, part)
    return oxdocx.Document.from_bytes(out.getvalue())


def bookmarks(doc):
    return [(bookmark.name, bookmark.text, bookmark.issue) for bookmark in doc.bookmarks]


def keeps_its_bookmark(doc):
    for read in (doc, oxdocx.Document.from_bytes(doc.to_bytes())):
        assert bookmarks(read) == [("_Toc217000001", "Introduction", None)]


def test_a_paragraph_inserted_before_a_google_heading_goes_before_its_bookmark():
    doc = led_heading()
    inserted = doc.insert_paragraph(HEADING, "New")
    assert inserted.text == "New"
    assert [paragraph.text for paragraph in doc.paragraphs] == ["Before", "New", "Introduction", "After"]
    keeps_its_bookmark(doc)

    doc = led_heading()
    inserted = doc.paragraphs[1].insert_paragraph_before("New")
    assert inserted.text == "New"
    keeps_its_bookmark(doc)


def test_a_table_inserted_before_a_google_heading_goes_before_its_bookmark():
    doc = led_heading()
    table = doc.insert_table(HEADING, 1, 1)
    table.cell(0, 0).text = "Cell"
    assert doc.tables[0].cell(0, 0).text == "Cell"
    keeps_its_bookmark(doc)


def test_a_google_heading_moves_and_goes_with_its_bookmark():
    doc = led_heading()
    doc.move_content(doc.paragraphs[1], 4)  # the end of the body
    assert [paragraph.text for paragraph in doc.paragraphs] == ["Before", "After", "Introduction"]
    keeps_its_bookmark(doc)

    doc = led_heading()
    assert doc.remove_content(HEADING)
    assert [paragraph.text for paragraph in doc.paragraphs] == ["Before", "After"]
    assert bookmarks(oxdocx.Document.from_bytes(doc.to_bytes())) == []

    doc = led_heading()
    assert doc.remove_content_range(HEADING, HEADING + 1) == 2
    assert [paragraph.text for paragraph in doc.paragraphs] == ["Before", "After"]
    assert bookmarks(oxdocx.Document.from_bytes(doc.to_bytes())) == []
