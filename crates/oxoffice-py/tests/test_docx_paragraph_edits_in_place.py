"""docx: the edits of one direct body paragraph of a body Word saved (`Paragraph.text`, `Paragraph.add_run`,
`Paragraph.add_equation`, `Run.remove`, `Run.add_field`, `Run.add_legacy_form_field`, `Document.split_run`) edit that
paragraph in place. They keep the handles the whole route keeps: the edited paragraph's handle stays and its runs
retire; every other paragraph, run, table, cell and section keeps reading and editing its own element. An edit that
leaves the paragraph as it was keeps every handle; one that is refused, or whose publication fails after the
paragraph was swapped in, leaves the document's bytes and every handle as they were. A field added before a later
hyperlink retires that hyperlink, as the item count of the body changes. Reads stay in document order after the
owners of an edit were removed by moving others into their keys."""

import pytest

import oxoffice.docx as oxdocx
from builders import edit_cost_docx

STALE = "stale"
EDITED = 3  # a body paragraph of six runs, Word's identifiers on each


def outcome(read):
    try:
        return read()
    except oxdocx.StaleElementError:
        return STALE


def debug_build():
    import oxoffice.docx._native as native

    return native._debug_build


@pytest.fixture(scope="module")
def body(tmp_path_factory):
    path = edit_cost_docx(tmp_path_factory.mktemp("in-place") / "body.docx", 25, probes=1)
    return path.read_bytes()


LEGACY = oxdocx.LegacyFormFieldDefinition.text_input(value="v", name="F1")

# Each call on the edited paragraph, and whether it changes how many story items the paragraph holds.
EDITS = {
    "Paragraph.text": (lambda doc, para: setattr(para, "text", "new text"), False),
    "Paragraph.add_run": (lambda doc, para: para.add_run(" added"), False),
    "Paragraph.add_equation": (lambda doc, para: para.add_equation("x^2", format="latex"), False),
    "Run.remove": (lambda doc, para: para.runs[1].remove(), False),
    "Run.add_field": (lambda doc, para: para.runs[0].add_field("PAGE", "1"), True),
    "Run.add_legacy_form_field": (lambda doc, para: para.runs[0].add_legacy_form_field(LEGACY), True),
    "Document.split_run": (lambda doc, para: doc.split_run(para, 0, 3), False),
}


def texts(doc):
    """Every read the edits must leave in document order, from fresh handles."""
    return {
        "paragraphs": [paragraph.text for paragraph in doc.paragraphs],
        "runs": [[run.text for run in paragraph.runs] for paragraph in doc.paragraphs],
        "tables": [[[cell.text for cell in row.cells] for row in table.rows] for table in doc.tables],
        "cell paragraphs": [
            [paragraph.text for paragraph in cell.paragraphs]
            for table in doc.tables
            for row in table.rows
            for cell in row.cells
        ],
        "story items": [(item.kind, item.text) for item in doc.story_items if item.story.kind == "body"],
    }


def reopened(doc):
    return oxdocx.Document.from_bytes(doc.to_bytes())


@pytest.mark.parametrize("call", list(EDITS))
def test_an_edit_in_place_keeps_every_handle_outside_its_paragraph(body, call):
    doc = oxdocx.Document.from_bytes(body)
    edit, _ = EDITS[call]
    fresh = {
        "edited": lambda: doc.paragraphs[EDITED],
        "before": lambda: doc.paragraphs[EDITED - 1],
        "after": lambda: doc.paragraphs[EDITED + 1],
        "after the table": lambda: doc.paragraphs[10],
        "cell": lambda: doc.tables[0].cell(1, 1).paragraphs[0],
    }
    paragraphs = {label: get() for label, get in fresh.items()}
    runs = {label: paragraph.runs[0] for label, paragraph in paragraphs.items()}
    table, cell, section = doc.tables[0], doc.tables[0].cell(1, 1), doc.section(1)

    returned = edit(doc, paragraphs["edited"])

    for label, paragraph in paragraphs.items():
        assert paragraph.text == fresh[label]().text, label
        expected = STALE if label == "edited" else fresh[label]().runs[0].text
        assert outcome(lambda: runs[label].text) == expected, label
    assert [row.cells[0].text for row in table.rows] == [row.cells[0].text for row in doc.tables[0].rows]
    assert cell.text == doc.tables[0].cell(1, 1).text
    assert section.page_width == doc.section(1).page_width
    if call == "Paragraph.add_run":
        assert returned.text == " added"
        returned.text = " changed"
        assert doc.paragraphs[EDITED].runs[-1].text == " changed"
    # Kept handles still edit their own element, and the edited paragraph takes further edits.
    runs["after"].text = "after, edited"
    assert doc.paragraphs[EDITED + 1].runs[0].text == "after, edited"
    runs["cell"].text = "cell, edited"
    assert doc.tables[0].cell(1, 1).paragraphs[0].text.startswith("cell, edited")
    paragraphs["edited"].add_run(" again")
    assert doc.paragraphs[EDITED].text.endswith(" again")
    assert texts(reopened(doc)) == texts(doc)


@pytest.mark.parametrize("call", ["Paragraph.text", "Document.split_run at the start", "Document.split_run at the end"])
def test_an_edit_in_place_that_changes_nothing_keeps_every_handle(body, call):
    doc = oxdocx.Document.from_bytes(body)
    # A paragraph Word saved whose only run holds the text it is set to: a run Word saved carries identifiers that
    # setting the text drops, so the first setting is an edit.
    if call == "Paragraph.text":
        doc.paragraphs[EDITED].text = "the same text"
    paragraph = doc.paragraphs[EDITED]
    run = paragraph.runs[0]
    other = doc.paragraphs[EDITED + 1].runs[2]
    before = doc.to_bytes()
    if call == "Paragraph.text":
        paragraph.text = paragraph.text
    elif call == "Document.split_run at the start":
        doc.split_run(paragraph, 0, 0)
    else:
        doc.split_run(paragraph, 0, len(run.text))
    assert doc.to_bytes() == before
    assert run.text == doc.paragraphs[EDITED].runs[0].text
    assert other.text == doc.paragraphs[EDITED + 1].runs[2].text


# Refusals that happen inside the edit of the paragraph's copy, after the in-place route took the call.
REFUSED_IN_THE_EDIT = {
    "Paragraph.text": lambda doc, para: setattr(para, "text", "a\x01b"),
    "Run.add_field": lambda doc, para: para.runs[0].add_field("PAGE\x01", ""),
    "Document.split_run past the text": lambda doc, para: doc.split_run(para, 0, 10_000),
    "Document.split_run past the runs": lambda doc, para: doc.split_run(para, 99, 1),
}
# Refusals of the arguments, before any route runs: they leave the document as it was too.
REFUSED_BEFORE_THE_EDIT = {
    "Paragraph.add_run": lambda doc, para: para.add_run("a\x01"),
    "Paragraph.add_equation": lambda doc, para: para.add_equation("\\frac{", format="latex"),
}
REFUSED = REFUSED_IN_THE_EDIT | REFUSED_BEFORE_THE_EDIT


def check_unchanged(doc, before, handles):
    assert doc.to_bytes() == before
    paragraph, runs, other = handles
    assert paragraph.text == doc.paragraphs[EDITED].text
    assert [run.text for run in runs] == [run.text for run in doc.paragraphs[EDITED].runs]
    assert other.text == doc.paragraphs[EDITED + 1].runs[0].text
    # The handles still edit their own element.
    runs[0].text = "still mine"
    assert doc.paragraphs[EDITED].runs[0].text == "still mine"


@pytest.mark.parametrize("call", list(REFUSED))
def test_a_refused_edit_in_place_leaves_the_bytes_and_every_handle_unchanged(body, call):
    doc = oxdocx.Document.from_bytes(body)
    paragraph = doc.paragraphs[EDITED]
    handles = (paragraph, list(paragraph.runs), doc.paragraphs[EDITED + 1].runs[0])
    before = doc.to_bytes()
    with pytest.raises((oxdocx.DocxError, ValueError)):
        REFUSED[call](doc, paragraph)
    check_unchanged(doc, before, handles)


@pytest.mark.skipif(not debug_build(), reason="a debug build fails the publication on request")
@pytest.mark.parametrize("failure", ["error", "panic"])
@pytest.mark.parametrize("call", list(EDITS))
def test_a_publication_that_fails_after_the_swap_puts_the_paragraph_back(body, call, failure, monkeypatch):
    doc = oxdocx.Document.from_bytes(body)
    paragraph = doc.paragraphs[EDITED]
    handles = (paragraph, list(paragraph.runs), doc.paragraphs[EDITED + 1].runs[0])
    before = doc.to_bytes()
    monkeypatch.setenv("OXOFFICE_DEBUG_FAIL_PARAGRAPH_SPLICE", failure)
    with pytest.raises(BaseException) as raised:
        EDITS[call][0](doc, paragraph)
    assert "on request" in str(raised.value)
    monkeypatch.delenv("OXOFFICE_DEBUG_FAIL_PARAGRAPH_SPLICE")
    check_unchanged(doc, before, handles)


# A field a run adds reads in place as part of that run's text, where the reopened save reads its result as a run
# of its own. A staged set_hyperlink_url reopened the document and hid it; the URL is now set in place.
FIELD_READS_OTHERWISE = [
    pytest.param(call, marks=pytest.mark.xfail(strict=True, reason="an added field's result reads otherwise after "
                                               "a reopen")) if call in ("Run.add_field", "Run.add_legacy_form_field")
    else call
    for call in EDITS
]


@pytest.mark.parametrize("call", FIELD_READS_OTHERWISE)
def test_a_field_added_before_a_later_hyperlink_retires_that_hyperlink_only(body, call):
    doc = oxdocx.Document.from_bytes(body)
    doc.paragraphs[EDITED - 1].add_hyperlink("earlier link", "https://example.com/earlier")
    doc.paragraphs[EDITED + 5].add_hyperlink("later link", "https://example.com/later")
    links = {link.text: link for link in doc.hyperlinks}
    edit, adds_items = EDITS[call]
    edit(doc, doc.paragraphs[EDITED])

    def moved(link):
        url = link.url + "?moved"
        doc.set_hyperlink_url(link, url)
        return [current.text for current in doc.hyperlinks if current.url == url]

    assert moved(links["earlier link"]) == ["earlier link"]
    assert outcome(lambda: moved(links["later link"])) == (STALE if adds_items else ["later link"])
    assert texts(reopened(doc)) == texts(doc)


def test_reads_stay_in_document_order_after_removals_that_moved_owners(body):
    doc = oxdocx.Document.from_bytes(body)
    # A table inserted, then paragraphs added in its cell: their owners come last in the inventory.
    table = doc.insert_table(5, 2, 2)
    cell = table.cell(0, 0)
    cell.paragraphs[0].text = "c1"
    cell.add_paragraph("c2")
    cell.add_paragraph("c3")
    kept = [cell.paragraphs[index] for index in range(3)]
    plain = doc.add_paragraph("plain one")
    # Each edit removes the runs of a paragraph, and the last owners move into their keys.
    for index in (EDITED, 1, EDITED + 1):
        doc.paragraphs[index].text = f"edited {index}"
    doc.paragraphs[0].runs[2].remove()
    slot = next(
        item.direct_body_index for item in doc.story_items if item.story.kind == "body" and item.text == "plain one"
    )
    assert doc.remove_content(slot)
    assert outcome(lambda: plain.text) == STALE
    assert [paragraph.text for paragraph in kept] == ["c1", "c2", "c3"]
    assert texts(reopened(doc)) == texts(doc)
    # A property edit that keeps every owner pairs the cell's paragraphs in document order.
    table.set_cell_grid_span(1, 0, None)
    assert [paragraph.text for paragraph in kept] == ["c1", "c2", "c3"]
    kept[1].text = "c2 edited"
    assert [paragraph.text for paragraph in doc.tables[0].cell(0, 0).paragraphs] == ["c1", "c2 edited", "c3"]
    assert texts(reopened(doc)) == texts(doc)


@pytest.mark.parametrize("call", FIELD_READS_OTHERWISE)
def test_an_edit_of_a_paragraph_that_holds_a_hyperlink_retires_that_hyperlink(body, call):
    doc = oxdocx.Document.from_bytes(body)
    doc.paragraphs[EDITED - 1].add_hyperlink("earlier link", "https://example.com/earlier")
    doc.paragraphs[EDITED].add_hyperlink("own link", "https://example.com/own")
    links = {link.text: link for link in doc.hyperlinks}
    after = doc.paragraphs[EDITED + 1].runs[0]
    EDITS[call][0](doc, doc.paragraphs[EDITED])

    def moved(link):
        url = link.url + "?moved"
        doc.set_hyperlink_url(link, url)
        return [current.text for current in doc.hyperlinks if current.url == url]

    assert outcome(lambda: moved(links["own link"])) == STALE
    assert moved(links["earlier link"]) == ["earlier link"]
    assert after.text == doc.paragraphs[EDITED + 1].runs[0].text
    assert texts(reopened(doc)) == texts(doc)


@pytest.mark.parametrize("call", list(EDITS))
def test_an_edit_of_a_paragraph_that_holds_a_text_box_retires_the_box_paragraph(call):
    from test_docx_block_handles import document, p, text_box, text_box_paragraph

    doc = document(p("first", " run") + p("box holder", text_box("in box"), " tail") + p("after", " run"), ids=True)
    box = text_box_paragraph(doc, "in box")
    first, after = doc.paragraphs[0].runs[0], doc.paragraphs[2].runs[0]
    EDITS[call][0](doc, doc.paragraphs[1])
    assert outcome(lambda: box.text) == STALE
    assert first.text == doc.paragraphs[0].runs[0].text
    assert after.text == doc.paragraphs[2].runs[0].text
    # A reopened field run reads joined to the run before it: compare the paragraphs.
    assert texts(reopened(doc))["paragraphs"] == texts(doc)["paragraphs"]


@pytest.mark.parametrize("call", ["Paragraph.text", "Paragraph.add_run", "Paragraph.add_equation"])
def test_an_edit_of_the_paragraph_that_ends_a_section_keeps_the_sections(body, call):
    doc = oxdocx.Document.from_bytes(body)
    ending = 25  # the empty paragraph that holds the first section's properties
    sections = [doc.section(index) for index in range(2)]
    widths = [section.page_width for section in sections]
    before = doc.paragraphs[ending - 1].runs[0]
    EDITS[call][0](doc, doc.paragraphs[ending])
    assert [section.page_width for section in sections] == widths
    assert before.text == doc.paragraphs[ending - 1].runs[0].text
    copy = reopened(doc)
    assert len(copy.sections) == len(doc.sections) == 2
    assert texts(copy) == texts(doc)


def test_removing_the_last_paragraph_after_edits_in_place_retires_its_runs():
    # Each edit in place removes the edited paragraph's runs, moving the last owners into their keys, and appends the
    # new runs: here the runs of the last paragraph end up before it in the inventory, and the removal of that plain
    # last paragraph (the tail removal) must still find them all.
    doc = oxdocx.Document()
    doc.add_paragraph("a").add_run("b")
    doc.add_paragraph("z")
    doc.paragraphs[1].add_run("y")
    doc.paragraphs[0].text = "new"
    last = doc.paragraphs[1]
    runs = list(last.runs)
    assert doc.remove_content(1)
    for run in runs:
        assert outcome(lambda: run.text) == STALE
    assert outcome(lambda: last.text) == STALE
    # A later edit publishes from the inventory the removal left (the debug oracles rebuild it).
    doc.paragraphs[0].add_run(" more")
    assert [paragraph.text for paragraph in doc.paragraphs] == ["new more"]
    assert texts(reopened(doc)) == texts(doc)


def test_a_property_edit_after_removals_that_moved_owners_keeps_each_text_box_handle_on_its_own_box():
    # Text box paragraphs share their owner key and have no parent: a property edit that keeps every owner pairs them
    # in document order, which their keys no longer give once removals moved owners into freed keys.
    from test_docx_block_handles import document, p, table, text_box, text_box_paragraph

    doc = document(
        p("a", " b", " c", " d", " e", " f")
        + p("one", text_box("box 1"))
        + p("two", text_box("box 2"))
        + p("three", text_box("box 3"))
        + table(p("cell"))
        + p("end", " x", " y", " z"),
        ids=True,
    )
    boxes = [text_box_paragraph(doc, f"box {k}") for k in (1, 2, 3)]
    doc.paragraphs[0].text = "edited"
    doc.paragraphs[4].text = "end edited"
    doc.tables[0].set_cell_grid_span(0, 0, 1)
    assert [box.text for box in boxes] == ["box 1", "box 2", "box 3"]
    boxes[0].text = "box 1 edited"
    assert [item.text for item in doc.story_items if item.story.kind == "text_box"] == ["box 1 edited", "box 2", "box 3"]
    assert texts(reopened(doc)) == texts(doc)
