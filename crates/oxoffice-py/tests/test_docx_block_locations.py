"""Block locations in the Python binding: every location argument takes the same forms, a paragraph of a block content
control and of every story takes a paragraph before it, and a refusal maps to its exception and names the route. The
Rust matrix (crates/oxoffice-docx/tests/docx/block_locations.rs) checks every operation at every location shape."""

import io
import re
import zipfile

import pytest

import oxoffice.docx as ox


def p(text):
    return f"<w:p><w:r><w:t>{text}</w:t></w:r></w:p>"


def sdt(tag, content):
    return f'<w:sdt><w:sdtPr><w:tag w:val="{tag}"/></w:sdtPr><w:sdtContent>{content}</w:sdtContent></w:sdt>'


TABLE = (
    '<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w="4000"/></w:tblGrid><w:tr><w:tc>'
    '<w:tcPr><w:tcW w:w="4000" w:type="dxa"/></w:tcPr>{}</w:tc></w:tr></w:tbl>'
)
# A text box in DrawingML alone, as LibreOffice and Google Docs write it.
TEXT_BOX = (
    '<w:r><w:drawing><wp:inline xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" '
    'distT="0" distB="0" distL="0" distR="0"><wp:extent cx="914400" cy="914400"/><wp:docPr id="7" name="Box"/>'
    '<a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:graphicData '
    'uri="http://schemas.microsoft.com/office/word/2010/wordprocessingShape"><wps:wsp '
    'xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape"><wps:cNvSpPr txBox="1"/>'
    '<wps:spPr><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></wps:spPr><wps:txbx><w:txbxContent>'
    f'{p("Text box")}</w:txbxContent></wps:txbx><wps:bodyPr/></wps:wsp></a:graphicData></a:graphic></wp:inline>'
    "</w:drawing></w:r>"
)
BODY = (
    p("Before")
    + sdt("box", p("Box 1") + sdt("inner", p("Inner 1")) + p("Box 2"))
    + p("After")
    + TABLE.format(p("Cell") + sdt("cellbox", p("Cell box 1")) + p("Cell end"))
    + sdt("tablebox", TABLE.format(p("Boxed cell")) + p("Under the table"))
    + p("Tail")
    + f"<w:p><w:r><w:t>Holder</w:t></w:r>{TEXT_BOX}</w:p>"
)


def with_parts(data, edits):
    """The package `data` with each part named in `edits` passed through its function."""
    out = io.BytesIO()
    with zipfile.ZipFile(io.BytesIO(data)) as source, zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as target:
        for item in source.infolist():
            part = source.read(item.filename)
            if item.filename in edits:
                part = edits[item.filename](part.decode("utf-8")).encode("utf-8")
            target.writestr(item, part)
    return out.getvalue()


def document(body=BODY):
    """`body` in a new document; with the fixture's body, a header holding a control, a footnote and a comment."""
    edit = lambda xml: re.sub(r"<w:body>.*?(<w:sectPr)", lambda m: f"<w:body>{body}{m.group(1)}", xml, flags=re.S)
    doc = ox.Document.from_bytes(with_parts(ox.Document().to_bytes(), {"word/document.xml": edit}))
    if body is not BODY:
        return doc
    doc.set_header("Head")
    doc.add_footnote(paragraph(doc, "Tail"), "Note")
    start = ox.RunPosition(body_index=0, run_index=0)
    doc.add_comment(ox.RunRange(start=start, end=ox.RunPosition(body_index=0, run_index=1)), author="Ada", text="Remark")
    header = next(s.part_name for s in doc.stories if s.kind == "header").lstrip("/")
    head = lambda xml: xml.replace("</w:hdr>", sdt("headbox", p("Head box 1")) + "</w:hdr>")
    return ox.Document.from_bytes(with_parts(doc.to_bytes(), {header: head}))


def paragraph(doc, text):
    """The paragraph handle whose text is `text`: in the body or a body table's cell, else a story item's."""
    found = [p for p in doc.paragraphs if p.text == text]
    found += [p for t in doc.tables for r in t.rows for c in r.cells for p in c.paragraphs if p.text == text]
    found = found or [i.paragraph for i in doc.story_items if i.kind == "paragraph" and i.text.strip() == text]
    assert len(found) == 1, (text, len(found))
    return found[0]


def texts(doc, kind):
    story = next(s for s in doc.stories if s.kind == kind)
    return [sp.text.strip() for sp in doc.story_paragraphs if (sp.story.kind, sp.story.owner_index) == (kind, story.owner_index)]


def body(doc):
    return next(s for s in doc.stories if s.kind == "body")


def test_the_issues_six_lines():
    issue = p("Before") + sdt("box", p("Inside 1") + p("Inside 2")) + p("After")
    loc = lambda doc, *path: ox.ContentLocation(body(doc), "paragraph", path)
    doc = document(issue)
    assert doc.paragraphs[2].insert_paragraph_before("New").text == "New"
    assert [x.text for x in doc.paragraphs] == ["Before", "Inside 1", "New", "Inside 2", "After"]
    doc = document(issue)
    with pytest.raises(ValueError, match="inside a block content control and has no body index: pass the paragraph"):
        doc.find_content_index(doc.paragraphs[2])
    doc.clone_content(doc.paragraphs[2], doc.paragraphs[2])
    doc.import_fragment(loc(doc, 1, 1), ox.DocumentFragment.from_range(doc, loc(doc, 1, 0), loc(doc, 1, 1)))
    for call in (
        lambda: doc.import_fragment(loc(doc, 1, 4), ox.DocumentFragment.from_range(doc, loc(doc, 1, 0), loc(doc, 1, 1))),
        lambda: ox.DocumentFragment.from_range(doc, loc(doc, 1, 3), loc(doc, 1, 4)),
        lambda: ox.DocumentFragment.from_range(doc, loc(doc, 1, 3), loc(doc, 2)),
    ):
        with pytest.raises(ox.DocxError, match=r"control_end\(story, 1\)"):
            call()
    end = ox.ContentLocation.control_end(body(doc), 1)
    assert end.is_end and end.index_path == (1,) and not ox.ContentLocation(body(doc), "content_control", (1,)).is_end
    doc.import_fragment(end, ox.DocumentFragment.from_range(doc, loc(doc, 1, 3), end))
    assert [x.text for x in doc.paragraphs] == ["Before", "Inside 1", "Inside 1", "Inside 2", "Inside 2", "Inside 2", "After"]


def test_a_location_argument_takes_every_form():
    after = lambda doc: next(i for i in doc.story_items if i.kind == "paragraph" and i.text == "After")
    path = lambda doc: ox.ContentLocation(body(doc), "paragraph", after(doc).index_path)
    for form in [after, lambda doc: paragraph(doc, "After"), lambda doc: 2, path]:
        doc = document()
        doc.clone_content(form(doc), body(doc))
        assert texts(doc, "body")[-1] == "After"
    doc = document()
    doc.clone_content(doc.tables[0], 2)
    assert len(doc.tables) == 3
    with pytest.raises(TypeError, match="must be an int, a Paragraph or Table, a StoryItem, a Story or a ContentLocation"):
        doc.clone_content("After", body(doc))
    with pytest.raises(OverflowError):
        doc.move_content(-1, body(doc))


def test_a_refusal_names_the_route():
    doc = document()
    boxed = next(t for t in doc.tables if t.cell(0, 0).text == "Boxed cell")
    for call, error, route in [
        (lambda: doc.find_content_index(paragraph(doc, "Cell")), ValueError, "in a table cell and has no body index"),
        (lambda: doc.find_content_index(boxed), ValueError, "inside a block content control and has no body index"),
        (lambda: doc.move_content(paragraph(doc, "Inner 1"), body(doc)), ox.DocxError, "remove_content_control"),
    ]:
        with pytest.raises(error, match=route):
            call()


STORIES = "body:Box 2,table_cell:Cell box 1,table_cell:Cell,header:Head,footnote:Note,comment:Remark,text_box:Text box"


@pytest.mark.parametrize(("kind", "text"), [pair.split(":") for pair in STORIES.split(",")])
def test_insert_paragraph_before_works_in_every_story(kind, text):
    doc = document()
    before = texts(doc, kind)
    target = doc.section(0).header.paragraphs[0] if kind == "header" else paragraph(doc, text)
    new = target.insert_paragraph_before("New", style="Heading 1")
    assert (new.text, new.style) == ("New", "Heading1")
    new.text = "New!"
    at = before.index(text)
    assert texts(ox.Document.from_bytes(doc.to_bytes()), kind) == before[:at] + ["New!"] + before[at:]


def test_a_paragraph_of_a_header_control_is_reached_by_its_location():
    # HeaderFooter.paragraphs lists the header's own paragraphs, as python-docx: a control's are reached by location.
    doc = document()
    located = {sp.text: ox.ContentLocation(sp.story, "paragraph", sp.index_path) for sp in doc.story_paragraphs}
    doc.clone_content(located["Head"], located["Head box 1"])
    assert texts(doc, "header") == ["Head", "Head", "Head box 1"]


def test_a_toc_entry_is_added_after_the_last_one():
    """A table of contents in a block control, its field begun in the first entry and ended in the last (what Word
    and Google Docs write): a copy of an entry goes before the one that ends the field, which takes a paragraph before
    it through its handle; one that holds the field's begin or end is not copied."""
    field = lambda kind: f'<w:r><w:fldChar w:fldCharType="{kind}"/></w:r>'
    entries = (
        f'<w:p>{field("begin")}<w:r><w:instrText> TOC \\o "1-3" </w:instrText></w:r>{field("separate")}'
        '<w:r><w:t>1 Introduction</w:t></w:r></w:p>'
        + p("2 Method")
        + f'<w:p><w:r><w:t>3 Results</w:t></w:r>{field("end")}</w:p>'
    )
    doc = document(p("Contents") + sdt("Table of Contents", entries) + p("Chapter"))
    toc = next(i for i in doc.story_items if i.kind == "content_control" and b"Table of Contents" in i.xml)
    entries = lambda: [sp for sp in doc.story_paragraphs if len(sp.index_path) == 2 and sp.index_path[0] == toc.index_path[0]]
    location = lambda sp: ox.ContentLocation(body(doc), "paragraph", sp.index_path)
    texts_before = [sp.text for sp in entries()]
    assert len(texts_before) == 3
    doc.clone_content(location(entries()[-2]), location(entries()[-1]))
    with pytest.raises(ox.DocxError, match="of a complex field"):
        doc.clone_content(location(entries()[-1]), location(entries()[-1]))
    # The paragraph that ends the field, through its handle: the body's story paragraphs are doc.paragraphs.
    body_paragraphs = [sp.index_path for sp in doc.story_paragraphs if sp.story.kind == "body"]
    assert len(body_paragraphs) == len(doc.paragraphs)
    doc.paragraphs[body_paragraphs.index(entries()[-1].index_path)].insert_paragraph_before("6 Appendix")
    after = [sp.text for sp in entries()]
    assert after == texts_before[:-1] + [texts_before[-2], "6 Appendix", texts_before[-1]]


def google(n, table):
    """`table` in a block control as Google Docs writes one: locked, alone."""
    return (
        f'<w:sdt><w:sdtPr><w:lock w:val="contentLocked"/><w:id w:val="{n}"/><w:tag w:val="goog_rdk_{n}"/></w:sdtPr>'
        f"<w:sdtContent>{table}</w:sdtContent></w:sdt>"
    )


def test_a_google_docs_table_in_a_locked_control_reads_and_edits_like_any_table():
    inner = TABLE.format(p("Cell") + google(3, TABLE.format(p("Inner {name}"))))
    doc = document(p("Before {name}") + google(1, TABLE.format(p("A {name}"))) + p("After {name}") + inner)
    doc.set_footer("Foot")
    footer = next(s.part_name for s in doc.stories if s.kind == "footer").lstrip("/")
    foot = lambda xml: xml.replace("</w:ftr>", google(2, TABLE.format(p("F {name}"))) + "</w:ftr>")
    doc = ox.Document.from_bytes(with_parts(doc.to_bytes(), {footer: foot}))
    # A loop over the paragraphs passes the table by, which is reached as a table, in the footer too.
    for paragraph in doc.paragraphs:
        paragraph.replace_text("{name}", "Ada")
    doc.tables[0].cell(0, 0).paragraphs[0].replace_text("{name}", "Ada")
    doc.section(0).footer.tables[0].cell(0, 0).paragraphs[0].replace_text("{name}", "Ada")
    # A table in a control inside a cell is among the cell's tables.
    doc.tables[1].cell(0, 0).tables[0].cell(0, 0).paragraphs[0].replace_text("{name}", "Ada")
    assert "Inner Ada" in doc.tables[1].cell(0, 0).tables[0].xml
    assert [x.text for x in doc.paragraphs] == ["Before Ada", "After Ada"]
    assert "A Ada" in doc.tables[0].xml and "A Ada" in doc.tables[0].cell(0, 0).xml
    doc.tables[0].replace_xml(doc.tables[0].xml.replace("A Ada", "A Bob"))
    doc.tables[0].add_row()
    assert len(doc.tables[0].rows) == 2
    # The handle is a location, as ContentLocation(story, "table", (c, k)) is; the control keeps a block.
    with pytest.raises(ox.DocxError, match="remove_content_control"):
        doc.pop_content(doc.tables[0])
    after = doc.pop_content(doc.paragraphs[1])
    doc.insert_content(doc.tables[0], after)
    doc.clone_content(doc.tables[0], body(doc))
    doc.move_content(ox.ContentLocation(body(doc), "table", (1, 0)), body(doc))
    assert [t.cell(0, 0).text for t in doc.tables] == ["Cell", "A Bob", "A Bob"]
    assert [x.text for x in doc.paragraphs] == ["Before Ada", "After Ada"]
    data = doc.to_bytes()
    with zipfile.ZipFile(io.BytesIO(data)) as package:
        xml = "".join(package.read(name).decode() for name in package.namelist() if name.endswith(".xml"))
    assert "F Ada" in xml and xml.count('<w:lock w:val="contentLocked"/>') == 3
