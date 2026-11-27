"""docx: an edit of one paragraph, or an insertion in one cell, retires the handles of what it changes and of what it
renumbers, and no others. Every other handle keeps reading and editing its own element: other paragraphs and their
runs, tables, cells, text boxes, headers and footers, sections, and the hyperlinks whose place the edit does not move.
A hyperlink is found by its story, the smallest story item that holds it and its index in that item, so an edit that
changes how many items its story holds retires the hyperlinks of the later items, and one that adds or removes a link
in a block content control retires the later links of that control; a text box or a table cell story is found by its
ordinal in its part, so removing a text box retires the later ones, and adding a table in a cell retires the
hyperlinks of the later cells. An edit that leaves the element as it was keeps every handle."""
import io
import itertools
import re
import zipfile

import pytest

import oxoffice.docx as oxdocx

STALE = "stale"

W14 = "http://schemas.microsoft.com/office/word/2010/wordml"
NAMESPACES = {
    "wp": "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing",
    "a": "http://schemas.openxmlformats.org/drawingml/2006/main",
    "wps": "http://schemas.microsoft.com/office/word/2010/wordprocessingShape",
    "w14": W14,
}
HYPERLINK = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink"


def p(*content):
    """A paragraph of runs (a string is a run of that text) and other inline XML."""
    return "<w:p>" + "".join(r(item) if not item.startswith("<") else item for item in content) + "</w:p>"


def r(text):
    return f'<w:r><w:t xml:space="preserve">{text}</w:t></w:r>'


def link(number, text):
    return f'<w:hyperlink r:id="rIdL{number}">{r(text)}</w:hyperlink>'


FIELD = '<w:fldSimple w:instr="PAGE">' + r("1") + "</w:fldSimple>"


def text_box(text):
    return (
        '<w:r><w:drawing><wp:anchor distT="0" distB="0" distL="0" distR="0" simplePos="0" relativeHeight="1" '
        'behindDoc="0" locked="0" layoutInCell="1" allowOverlap="1"><wp:simplePos x="0" y="0"/>'
        '<wp:positionH relativeFrom="column"><wp:posOffset>0</wp:posOffset></wp:positionH>'
        '<wp:positionV relativeFrom="paragraph"><wp:posOffset>0</wp:posOffset></wp:positionV>'
        '<wp:extent cx="914400" cy="457200"/><wp:effectExtent l="0" t="0" r="0" b="0"/><wp:wrapNone/>'
        '<wp:docPr id="DOCPR" name="Text Box"/><wp:cNvGraphicFramePr/><a:graphic>'
        '<a:graphicData uri="http://schemas.microsoft.com/office/word/2010/wordprocessingShape"><wps:wsp>'
        '<wps:cNvSpPr txBox="1"/><wps:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="914400" cy="457200"/></a:xfrm>'
        '<a:prstGeom prst="rect"><a:avLst/></a:prstGeom></wps:spPr><wps:txbx><w:txbxContent>'
        + p(text)
        + "</w:txbxContent></wps:txbx><wps:bodyPr/></wps:wsp></a:graphicData></a:graphic></wp:anchor>"
        "</w:drawing></w:r>"
    )


def table(*cells):
    """A table of one row, one cell per argument, each a string of block XML."""
    grid = "".join('<w:gridCol w:w="2000"/>' for _ in cells)
    return f"<w:tbl><w:tblGrid>{grid}</w:tblGrid><w:tr>" + "".join(f"<w:tc>{c}</w:tc>" for c in cells) + "</w:tr></w:tbl>"


def control(*paragraphs):
    return "<w:sdt><w:sdtPr/><w:sdtContent>" + "".join(paragraphs) + "</w:sdtContent></w:sdt>"


def google(n, table):
    """`table` in a block control as Google Docs writes one: locked, alone."""
    return (
        f'<w:sdt><w:sdtPr><w:lock w:val="contentLocked"/><w:id w:val="{n}"/><w:tag w:val="goog_rdk_{n}"/></w:sdtPr>'
        f"<w:sdtContent>{table}</w:sdtContent></w:sdt>"
    )


def document(body, footer="", ids=False):
    """A file whose body and footer hold the XML given, every `rIdLn` a hyperlink to example.com/n. With `ids`, every
    paragraph and run carries the identifiers Word writes."""
    if ids:
        counter = itertools.count(1)
        body, footer = (
            re.sub(
                "<w:p>",
                lambda _: f'<w:p w14:paraId="{next(counter):08X}" w14:textId="77777777" w:rsidR="00A1B2C3" '
                'w:rsidRDefault="00A1B2C3">',
                xml,
            ).replace("<w:r>", '<w:r w:rsidR="00A1B2C3">')
            for xml in (body, footer)
        )
    counter = itertools.count(1)
    body = re.sub("DOCPR", lambda _: str(next(counter)), body)
    base = oxdocx.Document()
    base.section(0).footer.add_paragraph("")
    out = io.BytesIO()
    with zipfile.ZipFile(io.BytesIO(base.to_bytes())) as source, zipfile.ZipFile(out, "w") as target:
        for item in source.infolist():
            part = source.read(item.filename).decode("utf-8")
            if item.filename == "word/document.xml":
                start = part.index("<w:body>") + len("<w:body>")
                part = part[:start] + body + part[part.index("<w:sectPr"):]
            elif item.filename == "word/footer1.xml" and footer:
                start = part.index(">", part.index("<w:ftr")) + 1
                part = part[:start] + footer + "</w:ftr>"
            elif item.filename == "word/_rels/document.xml.rels":
                part = part.replace("</Relationships>", links(body) + "</Relationships>")
            if item.filename in ("word/document.xml", "word/footer1.xml"):
                root = part.index(">", part.index("<w:", part.index("?>")))
                declared = part[:root]
                extra = "".join(f' xmlns:{k}="{v}"' for k, v in NAMESPACES.items() if f"xmlns:{k}=" not in declared)
                part = part[:root] + extra + part[root:]
            target.writestr(item, part)
        if footer:
            target.writestr(
                "word/_rels/footer1.xml.rels",
                '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
                '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
                + links(footer)
                + "</Relationships>",
            )
    return oxdocx.Document.from_bytes(out.getvalue())


def links(xml):
    return "".join(
        f'<Relationship Id="rIdL{n}" Type="{HYPERLINK}" Target="https://example.com/{n}" TargetMode="External"/>'
        for n in sorted(set(re.findall(r'r:id="rIdL(\d+)"', xml)), key=int)
    )


def outcome(read):
    """What a handle reads, or STALE when it raises the stale-handle error. Any other error fails the test."""
    try:
        return read()
    except oxdocx.StaleElementError:
        return STALE


def link_named(doc, text):
    found = [link for link in doc.hyperlinks if link.text == text]
    assert len(found) == 1, text
    return found[0]


def edits_its_own_link(doc, link):
    """Point the hyperlink `link` at a new address through its handle; the texts of the links that now carry it."""
    url = link.url + "?moved"
    doc.set_hyperlink_url(link, url)
    return [current.text for current in doc.hyperlinks if current.url == url]


def text_box_paragraph(doc, text):
    found = [
        item.paragraph
        for item in doc.story_items
        if item.story.kind == "text_box" and item.kind == "paragraph" and item.text == text
    ]
    assert len(found) == 1, text
    return found[0]


def check(doc, handles, links_kept, links_retired):
    """Each handle reads what a handle taken now reads (STALE where expected), and each kept hyperlink handle edits
    its own link while each retired one raises."""
    for label, (read, expected) in handles.items():
        assert outcome(read) == (STALE if expected is STALE else expected()), label
    for link in links_retired:
        assert outcome(lambda: edits_its_own_link(doc, link)) == STALE, link.text
    for link in links_kept:
        assert outcome(lambda: edits_its_own_link(doc, link)) == [link.text], link.text


IDS = pytest.mark.parametrize("ids", [False, True], ids=["plain", "word-ids"])

# The edit, and whether it changes how many story items the edited paragraph holds (a field is an item).
PARAGRAPH_EDITS = {
    "Paragraph.text": (lambda doc, para: setattr(para, "text", "new text"), False),
    "Paragraph.add_run": (lambda doc, para: para.add_run(" added"), False),
    "Paragraph.add_equation": (lambda doc, para: para.add_equation("x^2", format="latex"), False),
    "Run.remove": (lambda doc, para: para.runs[1].remove(), False),
    "Run.add_field": (lambda doc, para: para.runs[0].add_field("PAGE", "1"), True),
    "Run.add_legacy_form_field": (
        lambda doc, para: para.runs[0].add_legacy_form_field(
            oxdocx.LegacyFormFieldDefinition.text_input(value="v", name="F1")
        ),
        True,
    ),
    "Document.split_run": (lambda doc, para: doc.split_run(para, 0, 2), False),
}
PLACES = ["body", "cell", "footer"]


def neighbourhood(ids):
    """A body paragraph, a cell paragraph and a footer paragraph of two runs each, with hyperlinks before and after
    them in the body, in another cell and in the footer, a text box after them and a Google Docs table last."""
    return document(
        p("first", link(1, "before"))
        + p("edited", " second")
        + table(p("cell", " second"), p("other", link(2, "in cell")))
        + p("box holder", text_box("in box"))
        + p("after", link(3, "after"))
        + google(1, table(p("google"), p("google other"))),
        footer=p("foot", " second") + p("foot next", link(4, "in footer")),
        ids=ids,
    )


def placed(doc, place):
    """A fresh handle on the paragraph of `place` that the edits take."""
    return {
        "body": lambda: doc.paragraphs[1],
        "cell": lambda: doc.tables[0].cell(0, 0).paragraphs[0],
        "footer": lambda: doc.section(0).footer.paragraphs[0],
    }[place]()


@IDS
@pytest.mark.parametrize("place", PLACES)
@pytest.mark.parametrize("call", list(PARAGRAPH_EDITS))
def test_an_edit_of_one_paragraph_keeps_every_handle_outside_it(call, place, ids):
    if call == "Document.split_run" and place != "body":
        pytest.skip("Document.split_run takes a body paragraph")
    edit, adds_items = PARAGRAPH_EDITS[call]
    doc = neighbourhood(ids)
    fresh = {
        "first": lambda: doc.paragraphs[0],
        "body": lambda: doc.paragraphs[1],
        "box holder": lambda: doc.paragraphs[2],
        "after": lambda: doc.paragraphs[3],
        "cell": lambda: doc.tables[0].cell(0, 0).paragraphs[0],
        "other cell": lambda: doc.tables[0].cell(0, 1).paragraphs[0],
        "footer": lambda: doc.section(0).footer.paragraphs[0],
        "footer next": lambda: doc.section(0).footer.paragraphs[1],
        "in box": lambda: text_box_paragraph(doc, "in box"),
    }
    paragraphs = {label: get() for label, get in fresh.items()}
    runs = {label: paragraph.runs[0] for label, paragraph in paragraphs.items()}
    table_handle, cell_handle, other_cell = doc.tables[0], doc.tables[0].cell(0, 0), doc.tables[0].cell(0, 1)
    google_table, google_cell = doc.tables[1], doc.tables[1].cell(0, 1)
    section = doc.section(0)
    names = ["before", "in cell", "after", "in footer"]
    hyperlinks = {name: link_named(doc, name) for name in names}

    edit(doc, paragraphs[place])

    handles = {}
    for label, paragraph in paragraphs.items():
        handles[f"paragraph {label}"] = (lambda h=paragraph: h.text, lambda g=fresh[label]: g().text)
        # The edited paragraph's runs are born again; every other paragraph keeps its runs.
        run_expected = STALE if label == place else (lambda g=fresh[label]: g().runs[0].text)
        handles[f"run of {label}"] = (lambda h=runs[label]: h.text, run_expected)
    handles["table"] = (lambda: len(table_handle.rows), lambda: len(doc.tables[0].rows))
    handles["cell"] = (lambda: cell_handle.text, lambda: doc.tables[0].cell(0, 0).text)
    handles["other cell"] = (lambda: other_cell.text, lambda: doc.tables[0].cell(0, 1).text)
    handles["google table"] = (lambda: len(google_table.rows), lambda: len(doc.tables[1].rows))
    handles["google cell"] = (lambda: google_cell.text, lambda: doc.tables[1].cell(0, 1).text)
    handles["section"] = (lambda: section.page_width, lambda: doc.section(0).page_width)
    # A field added in a story renumbers the items after it there: the links of later items of that story retire.
    retired = set()
    if adds_items and place == "body":
        retired.add("after")
    if adds_items and place == "footer":
        retired.add("in footer")
    check(
        doc,
        handles,
        [hyperlinks[name] for name in names if name not in retired],
        [hyperlinks[name] for name in names if name in retired],
    )


@IDS
@pytest.mark.parametrize("removed", ["first", "last"])
def test_a_text_box_removed_retires_the_boxes_after_it_only(removed, ids):
    doc = document(
        p("zero", text_box("box A"))
        + p("one", text_box("box B"))
        + p("two", text_box("box C"))
        + p("three", link(1, "after")),
        ids=ids,
    )
    boxes = {name: text_box_paragraph(doc, name) for name in ("box A", "box B", "box C")}
    paragraphs = {index: doc.paragraphs[index] for index in range(4)}
    runs = {index: paragraph.runs[0] for index, paragraph in paragraphs.items()}
    after = link_named(doc, "after")
    edited = 0 if removed == "first" else 2

    paragraphs[edited].text = "no box"

    # The removed box is inside the edited paragraph; the ones after it are renumbered.
    gone = {"box A", "box B", "box C"} if removed == "first" else {"box C"}
    handles = {}
    for name, handle in boxes.items():
        expected = STALE if name in gone else (lambda n=name: text_box_paragraph(doc, n).text)
        handles[name] = (lambda h=handle: h.text, expected)
    for index in range(4):
        handles[f"paragraph {index}"] = (lambda h=paragraphs[index]: h.text, lambda i=index: doc.paragraphs[i].text)
        expected = STALE if index == edited else (lambda i=index: doc.paragraphs[i].runs[0].text)
        handles[f"run of {index}"] = (lambda h=runs[index]: h.text, expected)
    # The drawing was a story item of the body: the link of a later item is renumbered.
    check(doc, handles, [], [after])


@IDS
@pytest.mark.parametrize("insertion", ["Cell.add_table", "Cell.add_paragraph"])
def test_an_insertion_in_a_cell_retires_its_content_and_the_links_of_later_cells_only(insertion, ids):
    doc = document(
        table(p("a"), p("b", link(1, "in next cell")))
        + table(p("c", link(2, "in next table")))
        + p("after", link(3, "after")),
        ids=ids,
    )
    first, cell, next_cell = doc.tables[0], doc.tables[0].cell(0, 0), doc.tables[0].cell(0, 1)
    inside = cell.paragraphs[0]
    next_paragraph = next_cell.paragraphs[0]
    next_run = next_paragraph.runs[0]
    later = doc.tables[1].cell(0, 0).paragraphs[0]
    after = doc.paragraphs[0]
    after_run = after.runs[0]
    hyperlinks = {name: link_named(doc, name) for name in ("in next cell", "in next table", "after")}

    if insertion == "Cell.add_table":
        cell.add_table(1, 2)
    else:
        cell.add_paragraph("added")

    handles = {
        "table": (lambda: len(first.rows), lambda: len(doc.tables[0].rows)),
        "cell": (lambda: cell.text, lambda: doc.tables[0].cell(0, 0).text),
        "paragraph in the cell": (lambda: inside.text, STALE),
        "next cell": (lambda: next_cell.text, lambda: doc.tables[0].cell(0, 1).text),
        "paragraph of the next cell": (lambda: next_paragraph.text, lambda: doc.tables[0].cell(0, 1).paragraphs[0].text),
        "run of the next cell": (lambda: next_run.text, lambda: doc.tables[0].cell(0, 1).paragraphs[0].runs[0].text),
        "paragraph of the next table": (lambda: later.text, lambda: doc.tables[1].cell(0, 0).paragraphs[0].text),
        "paragraph after": (lambda: after.text, lambda: doc.paragraphs[0].text),
        "run after": (lambda: after_run.text, lambda: doc.paragraphs[0].runs[0].text),
    }
    # A nested table adds cell stories after this cell's: the links of the later cell stories are renumbered.
    renumbered = {"in next cell", "in next table"} if insertion == "Cell.add_table" else set()
    check(
        doc,
        handles,
        [link for name, link in hyperlinks.items() if name not in renumbered],
        [link for name, link in hyperlinks.items() if name in renumbered],
    )


@IDS
def test_removing_the_first_of_two_links_retires_the_other_link_of_its_paragraph_only(ids):
    doc = document(p("x", link(1, "first"), link(2, "second")) + p("after", link(3, "after")), ids=ids)
    edited, after = doc.paragraphs[0], doc.paragraphs[1]
    edited_run, after_run = edited.runs[0], after.runs[0]
    second, later = link_named(doc, "second"), link_named(doc, "after")

    edited.runs[1].remove()

    handles = {
        "edited paragraph": (lambda: edited.text, lambda: doc.paragraphs[0].text),
        "its run": (lambda: edited_run.text, STALE),
        "paragraph after": (lambda: after.text, lambda: doc.paragraphs[1].text),
        "run after": (lambda: after_run.text, lambda: doc.paragraphs[1].runs[0].text),
    }
    check(doc, handles, [later], [second])


@IDS
@pytest.mark.parametrize("story", ["footer", "cell"])
@pytest.mark.parametrize("edit", ["field removed", "field added"])
def test_a_field_removed_or_added_retires_the_links_of_later_paragraphs_of_its_story_only(edit, story, ids):
    # Two later linked paragraphs: a moved link lands on the route the other one had.
    later_paragraphs = p("see ", link(1, "later")) + p("also ", link(3, "last"))
    paragraphs = (p(FIELD, " page") if edit == "field removed" else p("page")) + later_paragraphs
    if story == "footer":
        doc = document(p("body", link(2, "body")), footer=paragraphs, ids=ids)
        fresh = lambda: doc.section(0).footer.paragraphs  # noqa: E731
    else:
        doc = document(table(paragraphs) + p("body", link(2, "body")), ids=ids)
        fresh = lambda: doc.tables[0].cell(0, 0).paragraphs  # noqa: E731
    edited, later, last = fresh()
    later_run = later.runs[0]
    body = doc.paragraphs[-1]
    later_link, last_link, body_link = (link_named(doc, name) for name in ("later", "last", "body"))

    if edit == "field removed":
        edited.text = "page"
    else:
        edited.runs[0].add_field("PAGE", "1")

    handles = {
        "edited paragraph": (lambda: edited.text, lambda: fresh()[0].text),
        "later paragraph": (lambda: later.text, lambda: fresh()[1].text),
        "its run": (lambda: later_run.text, lambda: fresh()[1].runs[0].text),
        "last paragraph": (lambda: last.text, lambda: fresh()[2].text),
        "body paragraph": (lambda: body.text, lambda: doc.paragraphs[-1].text),
    }
    check(doc, handles, [body_link], [later_link, last_link])


@IDS
@pytest.mark.parametrize("edit", ["Run.remove", "Paragraph.text"])
def test_a_link_removed_in_a_block_content_control_retires_its_later_links_only(edit, ids):
    doc = document(
        p("before", link(1, "before"))
        + control(p("s0 ", link(2, "s0")), p("s1 ", link(3, "s1")), p("s2 ", link(4, "s2")))
        + p("after", link(5, "after")),
        ids=ids,
    )
    paragraphs = list(doc.paragraphs)
    assert [paragraph.text for paragraph in paragraphs] == ["beforebefore", "s0 s0", "s1 s1", "s2 s2", "afterafter"]
    runs = [paragraph.runs[0] for paragraph in paragraphs]
    hyperlinks = {name: link_named(doc, name) for name in ("before", "s0", "s1", "s2", "after")}

    if edit == "Run.remove":
        paragraphs[2].runs[1].remove()
    else:
        paragraphs[2].text = "s1 "

    handles = {}
    for index, paragraph in enumerate(paragraphs):
        handles[f"paragraph {index}"] = (lambda h=paragraph: h.text, lambda i=index: doc.paragraphs[i].text)
        expected = STALE if index == 2 else (lambda i=index: doc.paragraphs[i].runs[0].text)
        handles[f"run of {index}"] = (lambda h=runs[index]: h.text, expected)
    # The control's links count together: the edited paragraph's link and the later ones of the control retire.
    retired = {"s1", "s2"}
    check(
        doc,
        handles,
        [link for name, link in hyperlinks.items() if name not in retired],
        [link for name, link in hyperlinks.items() if name in retired],
    )


@pytest.mark.parametrize("place", PLACES)
def test_setting_the_text_a_paragraph_already_has_keeps_every_handle(place):
    doc = neighbourhood(False)
    placed(doc, place).text = "same"
    paragraph = placed(doc, place)
    run = paragraph.runs[0]
    others = {
        "first": doc.paragraphs[0],
        "run of first": doc.paragraphs[0].runs[0],
        "run of other cell": doc.tables[0].cell(0, 1).paragraphs[0].runs[0],
        "run of footer next": doc.section(0).footer.paragraphs[1].runs[0],
    }
    expected = {label: handle.text for label, handle in others.items()}
    hyperlinks = [link_named(doc, name) for name in ("before", "in cell", "after", "in footer")]

    paragraph.text = "same"

    assert outcome(lambda: run.text) == "same"
    assert {label: outcome(lambda h=handle: h.text) for label, handle in others.items()} == expected
    for link in hyperlinks:
        assert outcome(lambda: edits_its_own_link(doc, link)) == [link.text], link.text


@IDS
@pytest.mark.parametrize("call", ["Paragraph.text", "Paragraph.add_run", "Run.add_field"])
def test_an_edit_in_a_text_box_retires_its_paragraph_and_keeps_the_paragraph_that_holds_it(call, ids):
    doc = document(
        p("zero", text_box("box A")) + p("one", text_box("box B")) + p("after", link(1, "after")), ids=ids
    )
    edited, other = text_box_paragraph(doc, "box A"), text_box_paragraph(doc, "box B")
    holder, holder_run = doc.paragraphs[0], doc.paragraphs[0].runs[0]
    after, after_run = doc.paragraphs[2], doc.paragraphs[2].runs[0]
    later = link_named(doc, "after")
    edit, _ = PARAGRAPH_EDITS[call]

    edit(doc, edited)

    handles = {
        # A text-box paragraph is rewritten through its story: its handle retires.
        "edited box paragraph": (lambda: edited.text, STALE),
        "other box": (lambda: other.text, lambda: text_box_paragraph(doc, "box B").text),
        "paragraph holding the box": (lambda: holder.text, lambda: doc.paragraphs[0].text),
        "its first run": (lambda: holder_run.text, lambda: doc.paragraphs[0].runs[0].text),
        "paragraph after": (lambda: after.text, lambda: doc.paragraphs[2].text),
        "run after": (lambda: after_run.text, lambda: doc.paragraphs[2].runs[0].text),
    }
    # The box's story is not the body's: the body link keeps its route.
    check(doc, handles, [later], [])


@IDS
@pytest.mark.parametrize("insertion", ["Cell.add_table", "Cell.add_paragraph"])
def test_the_cells_of_a_table_in_a_block_content_control_follow_their_table_when_cell_stories_move(insertion, ids):
    # A table in a block content control owns its cells, as any table: a table added in an earlier cell moves their
    # stories, and their content is found through the table. A control may hold two tables, which share its slot.
    doc = document(
        table(p("a"))
        + google(1, table(p("b"), p("c")))
        + control(table(p("e")), p("between"), table(p("f")))
        + table(p("d")),
        ids=ids,
    )
    in_controls = {
        text: next(item.paragraph for item in doc.story_items if item.kind == "paragraph" and item.text == text)
        for text in ("b", "c", "e", "f")
    }
    runs = {text: paragraph.runs[0] for text, paragraph in in_controls.items()}
    # `doc.tables` lists the tables in the controls too: the later table is the last one.
    later = doc.tables[-1].cell(0, 0).paragraphs[0]
    later_run = later.runs[0]

    cell = doc.tables[0].cell(0, 0)
    if insertion == "Cell.add_table":
        cell.add_table(1, 1)
    else:
        cell.add_paragraph("added")

    handles = {
        "paragraph of the later table": (lambda: later.text, lambda: doc.tables[-1].cell(0, 0).paragraphs[0].text),
        "its run": (lambda: later_run.text, lambda: doc.tables[-1].cell(0, 0).paragraphs[0].runs[0].text),
    }
    for text in in_controls:
        handles[f"paragraph {text}"] = (lambda h=in_controls[text]: h.text, lambda t=text: t)
        handles[f"run of {text}"] = (lambda h=runs[text]: h.text, lambda t=text: t)
    check(doc, handles, [], [])
    in_controls["f"].text = "f edited"
    assert [table.cell(0, 0).text for table in doc.tables][1:] == ["b", "e", "f edited", "d"]


def grid(name):
    """A 2x2 table whose cells read `name a`, `name b`, then `name c`, `name d`."""
    rows = "".join(
        "<w:tr>" + "".join(f"<w:tc>{p(f'{name} {cell}')}</w:tc>" for cell in row) + "</w:tr>" for row in ("ab", "cd")
    )
    return f'<w:tbl><w:tblGrid><w:gridCol w:w="2000"/><w:gridCol w:w="2000"/></w:tblGrid>{rows}</w:tbl>'


# Edits through a table handle.
TABLE_EDITS = {
    "Table.replace_xml": lambda t: t.replace_xml(t.xml.replace("x c", "x c edited")),
    "Table.add_row": lambda t: t.add_row(),
    "Table.remove_row": lambda t: t.remove_row(1),
    "Table.alignment": lambda t: setattr(t, "alignment", 1),
    "Cell.text": lambda t: setattr(t.cell(1, 0), "text", "edited"),
    "Cell.add_paragraph": lambda t: t.cell(1, 0).add_paragraph("added"),
    "Cell.add_table": lambda t: t.cell(1, 0).add_table(1, 1),
    "Cell.replace_xml": lambda t: t.cell(1, 0).replace_xml(t.cell(1, 0).xml.replace("x c", "edited")),
}


@pytest.mark.parametrize("place", ["body", "cell", "footer"])
@pytest.mark.parametrize("call", list(TABLE_EDITS))
def test_an_edit_through_a_google_docs_table_keeps_the_handles_a_direct_table_keeps(call, place):
    # The same edit through a direct table, in a twin document, tells which handles stay.
    def outcomes(wrap):
        edited = wrap(grid("x"))
        here = {"body": edited, "cell": table(p("holder") + edited), "footer": ""}[place]
        footer = p("foot") + (edited if place == "footer" else "")
        doc = document(p("before") + here + p("after") + table(p("other")), footer=footer)
        target = {
            "body": lambda: doc.tables[0],
            "cell": lambda: doc.tables[0].cell(0, 0).tables[0],
            "footer": lambda: doc.section(0).footer.tables[0],
        }[place]()
        taken = {
            "paragraph before": (doc.paragraphs[0], lambda h: h.text),
            "paragraph after": (doc.paragraphs[1], lambda h: h.text),
            "other table": (doc.tables[-1], lambda h: h.cell(0, 0).text),
            "its cell paragraph": (doc.tables[-1].cell(0, 0).paragraphs[0], lambda h: h.text),
            "footer paragraph": (doc.section(0).footer.paragraphs[0], lambda h: h.text),
            "edited table": (target, lambda h: h.cell(0, 1).text),
            "its other cell": (target.cell(0, 1), lambda h: h.text),
        }
        TABLE_EDITS[call](target)
        return {name: outcome(lambda h=handle, read=read: read(h)) for name, (handle, read) in taken.items()}

    google_outcomes = outcomes(lambda xml: google(1, xml))
    assert google_outcomes == outcomes(lambda xml: xml)
    assert google_outcomes["paragraph before"] == "before"


@pytest.mark.parametrize("call", ["Run.remove", "Run.add_field", "Paragraph.text"])
def test_an_edit_in_a_nested_block_content_control_still_works(call):
    doc = document(p("before") + control(p("outer", " two"), control(p("inner", " two"))) + p("after"))
    inner = next(paragraph for paragraph in doc.paragraphs if paragraph.text == "inner two")
    {
        "Run.remove": lambda: inner.runs[1].remove(),
        "Run.add_field": lambda: inner.runs[0].add_field("PAGE", "1"),
        "Paragraph.text": lambda: setattr(inner, "text", "inner"),
    }[call]()
    texts = [paragraph.text for paragraph in doc.paragraphs]
    assert texts[0] == "before" and texts[-1] == "after"
    assert texts[2] in ("inner", "inner1 two"), texts


NESTED_EDITS = {
    "Paragraph.add_run": (
        lambda para: para.add_run(" added"),
        lambda para: para.text == "edited two added",
    ),
    "Paragraph.add_equation": (
        lambda para: para.add_equation("x^2", format="latex"),
        lambda para: para.text == "edited two" and len(para.equations()) == 1,
    ),
    "Run.remove": (lambda para: para.runs[1].remove(), lambda para: para.text == "edited"),
    "Paragraph.replace_text": (
        lambda para: para.replace_text("ed tw", "ed, tw", expect=1),
        lambda para: para.text == "edited, two",
    ),
    "Run.add_field": (
        lambda para: para.runs[0].add_field("PAGE", "1"),
        lambda para: para.text == "edited1 two",
    ),
}


def edited_paragraph(doc):
    return next(paragraph for paragraph in doc.paragraphs if paragraph.text.startswith("edited"))


@pytest.mark.parametrize("call", NESTED_EDITS)
@pytest.mark.parametrize("depth", [1, 2, 3])
def test_a_run_edit_works_in_a_block_content_control_at_any_depth(call, depth):
    edited = p("edited", " two")
    for level in range(depth - 1):
        edited = control(p(f"level {level}"), edited)
    doc = document(p("before") + control(p("outer", " two"), edited, p("outer last")) + p("after"))
    para = edited_paragraph(doc)
    others = {
        paragraph.text: (paragraph, paragraph.runs[0], paragraph.runs[0].text)
        for paragraph in doc.paragraphs
        if paragraph.text in ("before", "outer two", "outer last", "after")
    }
    edit, edited_as_asked = NESTED_EDITS[call]
    edit(para)

    if call == "Paragraph.replace_text":
        # A replacement retires the paragraph handles, at any depth: the
        # others are fetched again.
        others = {
            paragraph.text: (paragraph, paragraph.runs[0], run_text)
            for paragraph in doc.paragraphs
            for text, (_, _, run_text) in others.items()
            if paragraph.text == text
        }
    else:
        assert edited_as_asked(para), para.text
    assert edited_as_asked(edited_paragraph(doc))
    assert edited_as_asked(edited_paragraph(oxdocx.Document.from_bytes(doc.to_bytes())))
    # Every other paragraph and run handle keeps reading and editing its own element.
    for text, (paragraph, run, run_text) in others.items():
        assert paragraph.text == text and run.text == run_text
        paragraph.add_run("!")
    reopened = oxdocx.Document.from_bytes(doc.to_bytes())
    texts = [paragraph.text for paragraph in reopened.paragraphs]
    for text in others:
        assert text + "!" in texts, texts


def plain_table_document(kind):
    """A table without hyperlinks, text boxes or nested tables between two body paragraphs, made through the API
    (new, or saved and opened again): `Paragraph.text` edits its cell paragraphs on the local cell route."""
    doc = oxdocx.Document()
    doc.add_paragraph("first").add_run(" second")
    cells = doc.add_table(1, 2)
    cells.cell(0, 0).text = "cell"
    cells.cell(0, 1).text = "other"
    doc.add_paragraph("last").add_run(" second")
    return oxdocx.Document.from_bytes(doc.to_bytes()) if kind == "reopened" else doc


LOCAL = pytest.mark.parametrize("kind", ["new", "reopened"])


@LOCAL
def test_a_cell_paragraph_edit_on_the_local_cell_route_keeps_the_runs_elsewhere(kind):
    doc = plain_table_document(kind)
    edited = doc.tables[0].cell(0, 0).paragraphs[0]
    edited_run = edited.runs[0]
    others = {
        "first": doc.paragraphs[0].runs[0],
        "other cell": doc.tables[0].cell(0, 1).paragraphs[0].runs[0],
        "last": doc.paragraphs[1].runs[0],
    }
    cell, table_handle = doc.tables[0].cell(0, 0), doc.tables[0]

    edited.text = "new text"

    handles = {
        "edited paragraph": (lambda: edited.text, lambda: "new text"),
        "its run": (lambda: edited_run.text, STALE),
        "cell": (lambda: cell.text, lambda: "new text"),
        "table": (lambda: len(table_handle.rows), lambda: 1),
    }
    for label, run in others.items():
        handles[f"run of {label}"] = (lambda h=run: h.text, lambda t=label.split()[0]: t)
    check(doc, handles, [], [])


@LOCAL
def test_setting_the_text_a_cell_paragraph_already_has_on_the_local_cell_route_keeps_every_handle(kind):
    doc = plain_table_document(kind)
    doc.tables[0].cell(0, 0).paragraphs[0].text = "same"
    paragraph = doc.tables[0].cell(0, 0).paragraphs[0]
    run = paragraph.runs[0]
    other = doc.paragraphs[0].runs[0]

    paragraph.text = "same"

    assert outcome(lambda: run.text) == "same"
    assert outcome(lambda: other.text) == "first"
