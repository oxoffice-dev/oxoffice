"""Block locations in Python: every call that inserts, copies, moves or removes whole blocks takes a location in the
same forms (ContentLocation, StoryItem, Paragraph or Table handle, int, Story), a paragraph inside a block content
control included, and a refusal names the route that works."""

import io
import re
import zipfile
from pathlib import Path

import pytest

import oxoffice.docx as ox

CORPUS = Path(__file__).resolve().parents[3] / "tests" / "corpus" / "files"


def p(text):
    return f"<w:p><w:r><w:t>{text}</w:t></w:r></w:p>"


def sdt(tag, content, properties=""):
    return (
        f'<w:sdt><w:sdtPr><w:tag w:val="{tag}"/>{properties}</w:sdtPr>'
        f"<w:sdtContent>{content}</w:sdtContent></w:sdt>"
    )


TABLE = (
    '<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w="4000"/></w:tblGrid><w:tr><w:tc>'
    '<w:tcPr><w:tcW w:w="4000" w:type="dxa"/></w:tcPr>{}</w:tc></w:tr></w:tbl>'
)

BODY = "".join(
    [
        p("Before"),
        sdt("box", p("Box 1") + sdt("inner", p("Inner 1")) + p("Box 2")),
        p("After"),
        TABLE.format(p("Cell") + sdt("cellbox", p("Cell box 1")) + p("Cell end")),
        sdt("typed", p("Plain"), "<w:text/>"),
        sdt("tablebox", TABLE.format(p("Boxed cell")) + p("Under the table")),
        p("Tail"),
    ]
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


_FIXTURE = []


def fixture():
    """A body with a block control holding a nested one, a table whose cell holds a control, a plain text control,
    a table in a control; a header holding a control; a footnote."""
    if not _FIXTURE:
        body = lambda xml: re.sub(r"<w:body>.*?(<w:sectPr)", lambda m: f"<w:body>{BODY}{m.group(1)}", xml, flags=re.S)
        doc = ox.Document.from_bytes(with_parts(ox.Document().to_bytes(), {"word/document.xml": body}))
        doc.set_header("Head")
        doc.add_footnote(paragraph(doc, "Tail"), "Note")
        data = doc.to_bytes()
        header = next(s.part_name for s in doc.stories if s.kind == "header").lstrip("/")
        data = with_parts(data, {header: lambda xml: xml.replace("</w:hdr>", sdt("headbox", p("Head box 1")) + "</w:hdr>")})
        _FIXTURE.append(data)
    return ox.Document.from_bytes(_FIXTURE[0])


def paragraph(doc, text):
    """The paragraph handle whose text is `text`, in the body or a cell of a body table."""
    found = [p for p in doc.paragraphs if p.text == text]
    for table in doc.tables:
        for row in table.rows:
            for cell in row.cells:
                found += [p for p in cell.paragraphs if p.text == text]
    assert len(found) == 1, (text, len(found))
    return found[0]


def story_of(doc, kind):
    return next(s for s in doc.stories if s.kind == kind)


def same(left, right):
    return (left.kind, left.part_name, left.owner_index) == (right.kind, right.part_name, right.owner_index)


def texts(doc, story):
    return [sp.text for sp in doc.story_paragraphs if same(sp.story, story)]


def story_location(doc, text):
    """The ContentLocation of the story paragraph whose text is `text`."""
    found = [sp for sp in doc.story_paragraphs if sp.text == text] or [
        sp for sp in doc.story_paragraphs if text in sp.text
    ]
    assert len(found) == 1, text
    return ox.ContentLocation(found[0].story, "paragraph", found[0].index_path)


def control(doc, tag):
    """The story and story item index of the content control tagged `tag`."""
    needle = f'<w:tag w:val="{tag}"/>'
    for item in doc.story_items:
        if item.kind == "content_control":
            xml = item.xml.decode("utf-8")
            if xml[xml.find("<w:tag ") :].startswith(needle):
                return item.story, item.index_path[0]
    raise AssertionError(tag)


def control_end(doc, tag):
    return ox.ContentLocation.control_end(*control(doc, tag))


def control_paragraph(doc, tag, k):
    story, index = control(doc, tag)
    return ox.ContentLocation(story, "paragraph", (index, k))


def body_story(doc):
    return story_of(doc, "body")


# Body paragraphs: Before, Box 1, Inner 1, Box 2, After, Plain, Under the table, Tail.
# Each shape: (name, the location argument, its story, where an insertion lands, the block's paragraphs, refusals).
END_REFUSALS = {"clone_from": "an end is a boundary", "move": "an end is a boundary", "pop": "an end is a boundary"}
SHAPES = [
    ("a body paragraph handle", lambda d: paragraph(d, "After"), body_story, 4, (4, 5), {}),
    ("a paragraph handle in a block control", lambda d: paragraph(d, "Box 2"), body_story, 3, (3, 4), {}),
    ("a ContentLocation [c, k]", lambda d: control_paragraph(d, "box", 1), body_story, 3, (3, 4), {}),
    ("the end of a block control", lambda d: control_end(d, "box"), body_story, 4, None, END_REFUSALS),
    (
        "a paragraph handle in a nested control",
        lambda d: paragraph(d, "Inner 1"),
        body_story,
        2,
        (2, 3),
        {"move": "only block of content control", "pop": "only block of content control"},
    ),
    ("the end of a nested control", lambda d: control_end(d, "inner"), body_story, 3, None, END_REFUSALS),
    ("the body Story, its end", body_story, body_story, 8, None, END_REFUSALS),
    ("an int body index", lambda d: 2, body_story, 4, (4, 5), {}),
    (
        "[c, n] past a control's paragraphs",
        lambda d: control_paragraph(d, "box", 2),
        body_story,
        0,
        None,
        dict.fromkeys(["insert_before", "clone_from", "clone_to", "move", "pop", "import"], "control_end(story, "),
    ),
    ("a cell paragraph handle", lambda d: paragraph(d, "Cell"), lambda d: story_of(d, "table_cell"), 0, (0, 1), {}),
    (
        "a paragraph handle of a control in a cell",
        lambda d: paragraph(d, "Cell box 1"),
        lambda d: story_of(d, "table_cell"),
        1,
        (1, 2),
        {"move": "only block of content control", "pop": "only block of content control"},
    ),
    ("a header paragraph", lambda d: story_location(d, "Head"), lambda d: story_of(d, "header"), 0, (0, 1), {}),
    (
        "a paragraph of a control in a header",
        lambda d: story_location(d, "Head box 1"),
        lambda d: story_of(d, "header"),
        1,
        (1, 2),
        {"move": "only block of content control", "pop": "only block of content control"},
    ),
    (
        "a footnote paragraph",
        lambda d: story_location(d, "Note"),
        lambda d: story_of(d, "footnote"),
        0,
        (0, 1),
        {"pop": "only block of its note"},
    ),
    (
        "the paragraph of a plain text control",
        lambda d: paragraph(d, "Plain"),
        body_story,
        5,
        (5, 6),
        {
            "insert_before": "set the control's value",
            "clone_to": "set the control's value",
            "import": "set the control's value",
            "move": "only block of content control",
            "pop": "only block of content control",
        },
    ),
    (
        "a table handle in a block control",
        lambda d: next(t for t in d.tables if t.cell(0, 0).text == "Boxed cell"),
        body_story,
        0,
        None,
        dict.fromkeys(["clone_from", "clone_to", "move", "pop", "import"], "#660"),
    ),
]

CALLS = ["insert_before", "clone_from", "clone_to", "move", "pop", "import"]


def before_fragment(doc):
    return ox.DocumentFragment.from_range(doc, paragraph(doc, "Before"), ox.ContentLocation(body_story(doc), "content_control", (1,)))


def call(name, doc, at, story):
    """Run the call `name` at the location argument `at`; `False` when it does not apply."""
    if name == "insert_before":
        if not isinstance(at, (ox.Paragraph, ox.ContentLocation)) or (isinstance(at, ox.ContentLocation) and at.is_end):
            return False
        target = at if isinstance(at, ox.Paragraph) else None
        if target is None:
            # A ContentLocation [c, k] names its paragraph: insert through the location calls.
            doc.clone_content(paragraph(doc, "Before"), at)
            return True
        target.insert_paragraph_before("New")
    elif name == "clone_from":
        doc.clone_content(at, story)
    elif name == "clone_to":
        doc.clone_content(paragraph(doc, "Before"), at)
    elif name == "move":
        doc.move_content(at, story)
    elif name == "pop":
        doc.pop_content(at)
    elif name == "import":
        doc.import_fragment(at, before_fragment(doc))
    return True


def expected(name, before, insert_at, block, inserted):
    after = list(before)
    if name in ("insert_before", "clone_to", "import"):
        after.insert(insert_at, inserted)
    elif name == "clone_from":
        after.extend(before[block[0] : block[1]])
    elif name == "move":
        del after[block[0] : block[1]]
        after.extend(before[block[0] : block[1]])
    elif name == "pop":
        del after[block[0] : block[1]]
    return after


CELLS = [(shape, name) for shape in SHAPES for name in CALLS]


@pytest.mark.parametrize(("shape", "name"), CELLS, ids=[f"{s[0]}-{n}" for s, n in CELLS])
def test_every_block_call_takes_every_location_form_or_names_the_working_route(shape, name):
    label, locate, story_at, insert_at, block, refusals = shape
    doc = fixture()
    story = story_at(doc)
    before = texts(doc, story)
    at = locate(doc)
    refusal = refusals.get(name)
    if refusal is not None:
        with pytest.raises((ox.DocxError, ValueError)) as caught:
            call(name, doc, at, story)
        assert refusal in str(caught.value)
        assert texts(doc, story_at(doc)) == before
        return
    if block is None and name in ("clone_from", "move", "pop"):
        pytest.fail(f"{label}: {name} at an end must be listed as a refusal")
    applied = call(name, doc, at, story)
    if not applied:
        return
    inserted = "New" if name == "insert_before" and isinstance(at, ox.Paragraph) else "Before"
    want = expected(name, before, insert_at, block, inserted)
    assert texts(doc, story_at(doc)) == want
    reopened = ox.Document.from_bytes(doc.to_bytes())
    assert texts(reopened, story_at(reopened)) == want


def test_the_issues_six_lines():
    doc = fixture()
    loc = lambda *path: ox.ContentLocation(body_story(doc), "paragraph", path)
    box = control(doc, "box")[1]

    new = paragraph(doc, "Box 2").insert_paragraph_before("New")
    assert new.text == "New"
    new.text = "New!"
    assert texts(doc, body_story(doc))[:5] == ["Before", "Box 1", "Inner 1", "New!", "Box 2"]

    doc = fixture()
    with pytest.raises(ValueError, match="inside a block content control and has no body index: pass the paragraph"):
        doc.find_content_index(paragraph(doc, "Box 2"))
    doc.clone_content(paragraph(doc, "Box 2"), paragraph(doc, "Box 2"))
    assert texts(doc, body_story(doc))[:5] == ["Before", "Box 1", "Inner 1", "Box 2", "Box 2"]

    doc = fixture()
    with pytest.raises(ox.DocxError, match=r"control_end\(story, 1\)"):
        doc.import_fragment(loc(box, 2), ox.DocumentFragment.from_range(doc, loc(box, 0), loc(box, 1)))
    with pytest.raises(ox.DocxError, match=r"control_end\(story, 1\)"):
        ox.DocumentFragment.from_range(doc, loc(box, 1), loc(box, 2))
    with pytest.raises(ox.DocxError, match=r"splits block ownership.*control_end\(story, 1\)"):
        ox.DocumentFragment.from_range(doc, loc(box, 1), paragraph(doc, "After"))
    end = ox.ContentLocation.control_end(body_story(doc), box)
    assert end.is_end and end.index_path == (box,) and end.item_kind == "content_control"
    assert not ox.ContentLocation(body_story(doc), "content_control", (box,)).is_end
    fragment = ox.DocumentFragment.from_range(doc, loc(box, 1), end)
    doc.import_fragment(end, fragment)
    assert texts(doc, body_story(doc))[:6] == ["Before", "Box 1", "Inner 1", "Box 2", "Box 2", "After"]


def test_insert_paragraph_before_works_on_every_paragraph():
    for text, story_kind in [("Inner 1", "body"), ("Cell box 1", "table_cell"), ("Cell", "table_cell"), ("Head", "header")]:
        doc = fixture()
        target = paragraph(doc, text) if story_kind != "header" else doc.section(0).header.paragraphs[0]
        story = story_of(doc, story_kind)
        before = texts(doc, story)
        new = target.insert_paragraph_before("New", style="Heading 1")
        assert new.text == "New"
        assert new.style == "Heading1"
        at = before.index(text)
        assert texts(doc, story_of(doc, story_kind)) == before[:at] + ["New"] + before[at:]


def test_find_content_index_names_the_route_for_every_other_block():
    doc = fixture()
    assert doc.find_content_index(paragraph(doc, "After")) == 2
    for text, where in [("Box 2", "inside a block content control"), ("Cell", "in a table cell")]:
        with pytest.raises(ValueError, match=f"{where} and has no body index: pass the paragraph or table itself"):
            doc.find_content_index(paragraph(doc, text))
    with pytest.raises(ValueError, match="#660"):
        doc.find_content_index(next(t for t in doc.tables if t.cell(0, 0).text == "Boxed cell"))


def test_a_location_argument_takes_every_form():
    def item(doc):
        return next(i for i in doc.story_items if i.kind == "paragraph" and i.text == "After")

    forms = [
        item,
        lambda doc: paragraph(doc, "After"),
        lambda doc: 2,
        lambda doc: ox.ContentLocation(body_story(doc), "paragraph", item(doc).index_path),
    ]
    for form in forms:
        doc = fixture()
        doc.clone_content(form(doc), body_story(doc))
        assert texts(doc, body_story(doc))[-1] == "After"
    doc = fixture()
    with pytest.raises(TypeError, match="must be an int, a Paragraph or Table, a StoryItem, a Story or a ContentLocation"):
        doc.clone_content("After", body_story(doc))
    with pytest.raises(OverflowError):
        doc.clone_content(-1, body_story(doc))


def test_a_toc_entry_is_added_after_the_last_one():
    for producer in ["word", "google"]:
        doc = ox.Document.from_bytes((CORPUS / producer / "thesis.docx").read_bytes())
        toc = next(i for i in doc.story_items if i.kind == "content_control" and b"Table of Contents" in i.xml)
        index = toc.index_path[0]
        entries = [sp for sp in doc.story_paragraphs if sp.index_path[:1] == (index,) and len(sp.index_path) == 2]
        last, field_end = entries[-2], entries[-1]
        location = lambda sp: ox.ContentLocation(body_story(doc), "paragraph", sp.index_path)
        doc.clone_content(location(last), location(field_end))
        after = [sp.text for sp in doc.story_paragraphs if sp.index_path[:1] == (index,) and len(sp.index_path) == 2]
        assert after == [sp.text for sp in entries[:-1]] + [last.text, field_end.text]
        # The first entry begins the TOC field and the last paragraph ends it: a copy of either is refused.
        entries = [sp for sp in doc.story_paragraphs if sp.index_path[:1] == (index,) and len(sp.index_path) == 2]
        field_end = entries[-1]
        for sp in (entries[0] if producer == "google" else entries[1], field_end):
            with pytest.raises(ox.DocxError, match="of a complex field"):
                doc.clone_content(location(sp), location(field_end))
        reopened = ox.Document.from_bytes(doc.to_bytes())
        assert [sp.text for sp in reopened.story_paragraphs if sp.index_path[:1] == (index,) and len(sp.index_path) == 2] == after
