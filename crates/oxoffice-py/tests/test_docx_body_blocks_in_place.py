"""docx: `Document.remove_content`, `remove_content_range` and `pop_content` remove direct body blocks of a body Word
saved in place, and publish the removal from the owners of the removed blocks alone. Handles to the removed blocks
raise; handles to every other paragraph, table, cell paragraph, section and hyperlink keep reading and editing their
own element, `Run` handles retire with the document epoch. Reads stay in document order after the removal moved owners
into the freed keys, and the document saves what the staged route saves. Blocks holding a comment, a note reference
or a section break take the staged route, which retires every handle. A publication that fails after the blocks were
removed puts them back: the bytes and every handle stay as they were."""

import pytest

import oxoffice.docx as oxdocx
from builders import edit_cost_docx

STALE = "stale"
REMOVED = 3  # a body paragraph of six runs, Word's identifiers on each


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
    path = edit_cost_docx(tmp_path_factory.mktemp("in-place") / "body.docx", 25, probes=1, picture_side=40)
    return path.read_bytes()


def slot(doc, element):
    return doc.find_content_index(element)


# Each call removing the block at a slot.
CALLS = {
    "Document.remove_content": lambda doc, at: doc.remove_content(at),
    "Document.remove_content_range": lambda doc, at: doc.remove_content_range(at, at + 1),
    "Document.pop_content": lambda doc, at: doc.pop_content(at),
}


def texts(doc):
    """Every read the removals must leave in document order, from fresh handles."""
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
        "hyperlinks": [(link.text, link.url) for link in doc.hyperlinks],
    }


def reopened(doc):
    return oxdocx.Document.from_bytes(doc.to_bytes())


@pytest.mark.parametrize("call", list(CALLS))
def test_a_removal_in_place_keeps_every_handle_outside_the_removed_block(body, call):
    doc = oxdocx.Document.from_bytes(body)
    removed = doc.paragraphs[REMOVED]
    removed_run = removed.runs[0]
    kept = {
        "before": doc.paragraphs[REMOVED - 1],
        "after": doc.paragraphs[REMOVED + 1],
        "after the table": doc.paragraphs[10],
        "cell": doc.tables[0].cell(1, 1).paragraphs[0],
    }
    expected = {label: paragraph.text for label, paragraph in kept.items()}
    run_after = kept["after"].runs[0]
    table, section = doc.tables[0], doc.section(1)
    rows = [[cell.text for cell in row.cells] for row in table.rows]
    width = section.page_width
    count = len(doc.paragraphs)

    CALLS[call](doc, slot(doc, removed))

    assert len(doc.paragraphs) == count - 1
    assert outcome(lambda: removed.text) == STALE
    assert outcome(lambda: removed_run.text) == STALE
    assert outcome(lambda: run_after.text) == STALE  # the document epoch retires
    assert {label: paragraph.text for label, paragraph in kept.items()} == expected
    assert kept["after"].text == doc.paragraphs[REMOVED].text
    assert [[cell.text for cell in row.cells] for row in table.rows] == rows
    assert section.page_width == width
    # Kept handles still edit their own element.
    kept["after"].text = "after, edited"
    assert doc.paragraphs[REMOVED].text == "after, edited"
    kept["cell"].text = "cell, edited"
    assert doc.tables[0].cell(1, 1).paragraphs[0].text == "cell, edited"
    table.cell(0, 0).text = "table, edited"
    assert doc.tables[0].cell(0, 0).text == "table, edited"
    assert texts(reopened(doc)) == texts(doc)


@pytest.mark.parametrize("call", ["Document.remove_content", "Document.pop_content"])
def test_a_table_removed_in_place_retires_its_own_handles_only(body, call):
    doc = oxdocx.Document.from_bytes(body)
    table, later = doc.tables[0], doc.tables[1]
    cell = table.cell(0, 1).paragraphs[0]
    later_cell = later.cell(1, 0).paragraphs[0]
    before, after = doc.paragraphs[9], doc.paragraphs[10]
    expected = (before.text, after.text, later_cell.text)
    CALLS[call](doc, slot(doc, table))
    assert outcome(lambda: table.cell(0, 0).text) == STALE
    assert outcome(lambda: cell.text) == STALE
    assert (before.text, after.text, later_cell.text) == expected
    assert doc.tables[0].cell(1, 0).paragraphs[0].text == later_cell.text
    later_cell.text = "later, edited"
    assert doc.tables[0].cell(1, 0).text == "later, edited"
    assert texts(reopened(doc)) == texts(doc)


def test_a_range_of_paragraphs_and_a_table_is_removed_in_place(body):
    doc = oxdocx.Document.from_bytes(body)
    first = doc.paragraphs[8]
    after = doc.paragraphs[11]
    later = doc.tables[1]
    start = slot(doc, first)
    end = slot(doc, after)
    assert doc.remove_content_range(start, end) == end - start
    assert outcome(lambda: first.text) == STALE
    assert after.text == doc.paragraphs[8].text
    assert later.cell(0, 0).text == doc.tables[0].cell(0, 0).text
    after.add_run(" more")
    assert doc.paragraphs[8].text.endswith(" more")
    assert texts(reopened(doc)) == texts(doc)


@pytest.mark.parametrize("call", list(CALLS))
def test_hyperlinks_after_a_removal_in_place_keep_editing_their_link(body, call):
    doc = oxdocx.Document.from_bytes(body)
    doc.paragraphs[REMOVED - 1].add_hyperlink("earlier link", "https://example.com/earlier")
    doc.paragraphs[REMOVED].add_hyperlink("own link", "https://example.com/own")
    doc.paragraphs[REMOVED + 5].add_hyperlink("later link", "https://example.com/later")
    doc.tables[1].cell(0, 0).paragraphs[0].add_hyperlink("cell link", "https://example.com/cell")
    links = {link.text: link for link in doc.hyperlinks}
    CALLS[call](doc, slot(doc, doc.paragraphs[REMOVED]))

    def moved(link):
        url = link.url + "?moved"
        doc.set_hyperlink_url(link, url)
        return [current.text for current in doc.hyperlinks if current.url == url]

    assert outcome(lambda: moved(links["own link"])) == STALE
    for text in ("earlier link", "later link", "cell link"):
        assert moved(links[text]) == [text]
    assert texts(reopened(doc)) == texts(doc)


@pytest.mark.parametrize("call", list(CALLS))
def test_a_block_content_control_is_removed_in_place(body, call):
    doc = oxdocx.Document.from_bytes(body)
    spot = next(paragraph for paragraph in doc.paragraphs if paragraph.text == "spot 0")
    control = next(paragraph for paragraph in doc.paragraphs if paragraph.text == "control 0")
    before = doc.paragraphs[REMOVED]
    expected = before.text
    CALLS[call](doc, slot(doc, spot) - 1)
    assert outcome(lambda: control.text) == STALE
    assert spot.text == "spot 0"
    assert before.text == expected
    spot.text = "spot, edited"
    assert "spot, edited" in [paragraph.text for paragraph in doc.paragraphs]
    assert texts(reopened(doc)) == texts(doc)


def staged_targets(doc):
    """Blocks the staged route reconciles: a commented paragraph, a paragraph holding a note reference, and the
    paragraph that ends the first section."""
    commented = next(paragraph for paragraph in doc.paragraphs if paragraph.text == "commented 0")
    noted = doc.paragraphs[REMOVED + 2]
    doc.add_footnote(noted, "a note")
    return {"comment": commented, "note": noted, "section": doc.paragraphs[25]}


@pytest.mark.parametrize("target", ["comment", "note", "section"])
@pytest.mark.parametrize("call", list(CALLS))
def test_blocks_the_staged_route_reconciles_keep_it(body, call, target, monkeypatch):
    results = []
    for staged in (False, True):
        if staged:
            if not debug_build():
                break
            monkeypatch.setenv("OXOFFICE_DEBUG_STAGED_BODY_REMOVAL", "1")
        doc = oxdocx.Document.from_bytes(body)
        removed = staged_targets(doc)[target]
        other = doc.paragraphs[REMOVED]
        sections = len(doc.sections)
        if call == "Document.pop_content" and target != "note":
            # A fragment owns no comment thread and no section: the staged route refuses, changing nothing.
            before = doc.to_bytes()
            with pytest.raises(oxdocx.DocxError):
                CALLS[call](doc, slot(doc, removed))
            assert doc.to_bytes() == before
            assert other.text == doc.paragraphs[REMOVED].text
            results.append(before)
            continue
        CALLS[call](doc, slot(doc, removed))
        # The staged route retires every handle.
        assert outcome(lambda: other.text) == STALE
        assert len(doc.sections) == sections - (target == "section")
        assert texts(reopened(doc)) == texts(doc)
        results.append(doc.to_bytes())
    monkeypatch.delenv("OXOFFICE_DEBUG_STAGED_BODY_REMOVAL", raising=False)
    assert len(set(results)) == 1


@pytest.mark.skipif(not debug_build(), reason="a debug build takes the staged route on request")
@pytest.mark.parametrize("element", ["paragraph", "table", "picture"])
@pytest.mark.parametrize("call", list(CALLS))
def test_a_removal_in_place_saves_what_the_staged_route_saves(body, call, element, monkeypatch):
    saved = []
    for staged in (False, True):
        if staged:
            monkeypatch.setenv("OXOFFICE_DEBUG_STAGED_BODY_REMOVAL", "1")
        doc = oxdocx.Document.from_bytes(body)
        target = {
            "paragraph": lambda: doc.paragraphs[REMOVED],
            "table": lambda: doc.tables[0],
            "picture": lambda: doc.paragraphs[9],  # every 10th paragraph holds a picture
        }[element]()
        CALLS[call](doc, slot(doc, target))
        saved.append((doc.to_bytes(), texts(doc)))
    monkeypatch.delenv("OXOFFICE_DEBUG_STAGED_BODY_REMOVAL")
    assert saved[0][1] == saved[1][1]
    assert saved[0][0] == saved[1][0]


@pytest.mark.skipif(not debug_build(), reason="a debug build fails the publication on request")
@pytest.mark.parametrize("failure", ["error", "panic"])
@pytest.mark.parametrize("call", list(CALLS))
def test_a_publication_that_fails_after_the_removal_puts_the_blocks_back(body, call, failure, monkeypatch):
    doc = oxdocx.Document.from_bytes(body)
    removed = doc.paragraphs[REMOVED]
    runs = list(removed.runs)
    other = doc.paragraphs[REMOVED + 1].runs[0]
    before = doc.to_bytes()
    monkeypatch.setenv("OXOFFICE_DEBUG_FAIL_BLOCK_SPLICE", failure)
    with pytest.raises(BaseException) as raised:
        CALLS[call](doc, slot(doc, removed))
    assert "on request" in str(raised.value)
    monkeypatch.delenv("OXOFFICE_DEBUG_FAIL_BLOCK_SPLICE")
    assert doc.to_bytes() == before
    assert removed.text == doc.paragraphs[REMOVED].text
    assert [run.text for run in runs] == [run.text for run in doc.paragraphs[REMOVED].runs]
    assert other.text == doc.paragraphs[REMOVED + 1].runs[0].text
    runs[0].text = "still mine"
    assert doc.paragraphs[REMOVED].runs[0].text == "still mine"


def test_reads_stay_in_document_order_after_removals_that_moved_owners(body):
    doc = oxdocx.Document.from_bytes(body)
    # A table inserted, then paragraphs added in its cell: their owners come last in the inventory, and each removal
    # moves the last owners into the keys it frees.
    table = doc.insert_table(5, 2, 2)
    cell = table.cell(0, 0)
    cell.paragraphs[0].text = "c1"
    cell.add_paragraph("c2")
    cell.add_paragraph("c3")
    kept = [cell.paragraphs[index] for index in range(3)]
    doc.paragraphs[12].add_hyperlink("link", "https://example.com/link")
    for index in (REMOVED, 1, 0):
        doc.remove_content(slot(doc, doc.paragraphs[index]))
    doc.pop_content(slot(doc, doc.paragraphs[3]))
    assert [paragraph.text for paragraph in kept] == ["c1", "c2", "c3"]
    kept[1].text = "c2 edited"
    assert [paragraph.text for paragraph in doc.tables[0].cell(0, 0).paragraphs] == ["c1", "c2 edited", "c3"]
    assert texts(reopened(doc)) == texts(doc)


@pytest.mark.parametrize("call", ["Document.remove_content", "Document.pop_content"])
def test_a_story_item_after_a_removal_in_place_is_taken_again(body, call):
    doc = oxdocx.Document.from_bytes(body)
    CALLS[call](doc, slot(doc, doc.paragraphs[REMOVED]))
    item = next(item for item in doc.story_items if item.story.kind == "body" and item.direct_body_index == REMOVED)
    text = item.text
    fragment = doc.pop_content(item)
    assert text not in [paragraph.text for paragraph in doc.paragraphs][REMOVED:REMOVED + 1]
    doc.insert_content(REMOVED, fragment)
    assert doc.paragraphs[REMOVED].text == text
    assert texts(reopened(doc)) == texts(doc)


def test_a_picture_popped_in_place_and_inserted_again_keeps_unique_drawing_ids(body):
    # The removed picture's drawing id stays reserved until the document is reopened: the copy inserted again takes
    # another one, where the staged route, which reopens, gave it back its own.
    import re
    import zipfile
    import io

    doc = oxdocx.Document.from_bytes(body)
    fragment = doc.pop_content(slot(doc, doc.paragraphs[9]))
    doc.insert_content(0, fragment)
    xml = zipfile.ZipFile(io.BytesIO(doc.to_bytes())).read("word/document.xml").decode()
    ids = re.findall(r'<wp:docPr id="(\d+)"', xml)
    assert ids and len(ids) == len(set(ids))
    assert texts(reopened(doc)) == texts(doc)


def staged_and_local(make, edit, monkeypatch):
    """What `edit` returns or raises and what the document then saves, on the route in place and on the staged
    route, which a debug build takes on request."""
    results = []
    for staged in (False, True):
        if staged:
            monkeypatch.setenv("OXOFFICE_DEBUG_STAGED_BODY_REMOVAL", "1")
        doc = make()
        try:
            returned = edit(doc)
        except oxdocx.DocxError as error:
            returned = f"raised {error}"
        try:
            saved = doc.to_bytes()
        except oxdocx.DocxError as error:
            saved = f"save raised {error}"
        results.append((repr(returned) if not hasattr(returned, "kind") else "fragment", saved, doc))
    monkeypatch.delenv("OXOFFICE_DEBUG_STAGED_BODY_REMOVAL")
    return results


BOOKMARKED = {
    # The bookmark start before the paragraph whose end it holds goes with it, and so does the end after it.
    "owned around": '<w:bookmarkStart w:id="1" w:name="a"/>' + "{p}" + '<w:bookmarkEnd w:id="1"/>',
    # Between the paragraph and the start it owns, a start whose bookmark ends further on stays: the markers that go
    # are not one range, and the staged route takes them.
    "split by a start that stays": '<w:bookmarkStart w:id="3" w:name="c"/><w:bookmarkStart w:id="2" w:name="b"/>'
    + "{p}"
    + '<w:bookmarkEnd w:id="3"/>',
}


@pytest.mark.skipif(not debug_build(), reason="a debug build takes the staged route on request")
@pytest.mark.parametrize("shape", list(BOOKMARKED))
@pytest.mark.parametrize("call", list(CALLS))
def test_bookmark_markers_go_with_their_block_as_the_staged_route_takes_them(call, shape, monkeypatch):
    from test_docx_block_handles import document, p

    xml = p("first", " run") + BOOKMARKED[shape].format(p=p("marked", " run")) + p("later", " run")
    if shape == "split by a start that stays":
        xml += p('<w:bookmarkEnd w:id="2"/>', "end")
    results = staged_and_local(
        lambda: document(xml, ids=True),
        lambda doc: CALLS[call](doc, slot(doc, doc.paragraphs[1])),
        monkeypatch,
    )
    (local, local_bytes, doc), (staged, staged_bytes, _) = results
    assert (local, local_bytes) == (staged, staged_bytes)
    assert texts(reopened(doc)) == texts(doc)


@pytest.mark.parametrize("call", list(CALLS))
def test_a_paragraph_holding_a_text_box_is_removed_with_every_other_box_kept(call):
    from test_docx_block_handles import document, p, table, text_box, text_box_paragraph

    doc = document(
        p("first", " run")
        + p("one", text_box("box 1"))
        + p("two", text_box("box 2"))
        + table(p("cell"))
        + p("end", " x"),
        ids=True,
    )
    box, cell = text_box_paragraph(doc, "box 2"), doc.tables[0].cell(0, 0).paragraphs[0]
    CALLS[call](doc, slot(doc, doc.paragraphs[1]))
    assert [item.text for item in doc.story_items if item.story.kind == "text_box"] == ["box 2"]
    # A block holding a text box keeps the whole route, which retires every handle.
    assert outcome(lambda: box.text) == STALE
    assert outcome(lambda: cell.text) == STALE
    assert texts(reopened(doc)) == texts(doc)


@pytest.mark.parametrize("call", list(CALLS))
def test_a_table_before_another_one_is_removed_where_a_text_box_holds_cell_stories_unanchored(call):
    from test_docx_block_handles import document, p, table, text_box

    doc = document(
        p("box holder", text_box("in box"))
        + table(p("first cell"))
        + p("between", " run")
        + table(p("second cell"))
        + p("end", " x"),
        ids=True,
    )
    second = doc.tables[1].cell(0, 0).paragraphs[0]
    CALLS[call](doc, slot(doc, doc.tables[0]))
    assert [table.cell(0, 0).text for table in doc.tables] == ["second cell"]
    # Cell stories a text box holds are not anchored to their items: the whole route, which retires every handle.
    assert outcome(lambda: second.text) == STALE
    assert texts(reopened(doc)) == texts(doc)


@pytest.mark.skipif(not debug_build(), reason="a debug build takes the staged route on request")
@pytest.mark.parametrize("call", list(CALLS))
def test_a_body_a_typed_edit_cannot_save_takes_the_staged_route(call, tmp_path, monkeypatch):
    from builders import body_namespace_docx

    data = body_namespace_docx(tmp_path / "namespaced.docx").read_bytes()
    results = staged_and_local(
        lambda: oxdocx.Document.from_bytes(data),
        lambda doc: CALLS[call](doc, 0),
        monkeypatch,
    )
    assert [result[:2] for result in results[:1]] == [result[:2] for result in results[1:]]


def nested_body_items(doc):
    """Body story items inside a direct body block, which name the slot of the block that holds them, one per kind:
    a drawing in its paragraph, and a content control nested in a block that is one."""
    seen, nested = set(), {}
    for item in doc.story_items:
        if item.story.kind != "body" or item.direct_body_index is None:
            continue
        if item.direct_body_index in seen:
            nested.setdefault(item.kind, item)
        seen.add(item.direct_body_index)
    return nested


@pytest.mark.skipif(not debug_build(), reason="a debug build takes the staged route on request")
@pytest.mark.parametrize("kind", ["drawing", "content_control"])
def test_popping_an_item_inside_a_block_takes_what_the_staged_route_takes(body, kind, monkeypatch):
    assert kind in nested_body_items(oxdocx.Document.from_bytes(body))
    local, staged = staged_and_local(
        lambda: oxdocx.Document.from_bytes(body),
        lambda doc: doc.pop_content(nested_body_items(doc)[kind]),
        monkeypatch,
    )
    assert local[:2] == staged[:2]
