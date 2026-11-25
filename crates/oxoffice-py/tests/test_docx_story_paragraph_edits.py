"""docx: the story calls that edit one paragraph (`Document.set_story_text`, `Document.insert_legacy_form_field`, and
`Document.add_story_permission_range` with both ends in one paragraph) edit a direct body paragraph in place, as the
paragraph edits do: the paragraph's handle stays and its runs retire, every other paragraph, run, table, cell and
section keeps reading and editing its own element. A refused call, and a publication that fails after the paragraph
was swapped in, leave the document's bytes and every handle as they were. A paragraph of another story (a header, a
table cell) takes the staged route and reads the same. A permission range takes the smallest free id, and a refused
one gives it back."""

import pytest

import oxoffice.docx as oxdocx
from test_docx_paragraph_edits_in_place import (EDITED, STALE, body, check_unchanged, debug_build, outcome, reopened,
                                                texts)

__all__ = ["body"]  # the fixture

LEGACY = oxdocx.LegacyFormFieldDefinition.text_input(value="v", name="F1")


def item(doc, paragraph):
    (found,) = [it for it in doc.story_items if it.story.kind == "body" and it.kind == "paragraph"
                and it.text == paragraph.text]
    return found


def one_paragraph_range(it, start=0, end=1):
    return oxdocx.StoryRunRange(start=oxdocx.StoryRunPosition(item=it, run_index=start),
                                end=oxdocx.StoryRunPosition(item=it, run_index=end))


CALLS = {
    "Document.set_story_text": lambda doc, it: doc.set_story_text(it, "omega"),
    "Document.insert_legacy_form_field": lambda doc, it: doc.insert_legacy_form_field(
        oxdocx.StoryRunPosition(item=it, run_index=1), LEGACY),
    "Document.add_story_permission_range": lambda doc, it: doc.add_story_permission_range(
        one_paragraph_range(it), editor="Clerk"),
}

EXPECTED = {
    "Document.set_story_text": lambda paragraph: paragraph.text == "omega",
    "Document.insert_legacy_form_field": lambda paragraph: True,
    "Document.add_story_permission_range": lambda paragraph: True,
}

MARKERS = {
    "Document.set_story_text": b"omega",
    "Document.insert_legacy_form_field": b'<w:name w:val="F1"/>',
    "Document.add_story_permission_range": b'w:ed="Clerk"',
}


def body_xml(doc):
    import io
    import zipfile

    with zipfile.ZipFile(io.BytesIO(doc.to_bytes())) as package:
        return package.read("word/document.xml")


@pytest.mark.parametrize("call", list(CALLS))
def test_a_story_paragraph_call_keeps_every_handle_outside_its_paragraph(body, call):
    doc = oxdocx.Document.from_bytes(body)
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
    header = doc.section(0).header.paragraphs[0]

    CALLS[call](doc, item(doc, paragraphs["edited"]))

    assert EXPECTED[call](doc.paragraphs[EDITED])
    for label, paragraph in paragraphs.items():
        assert paragraph.text == fresh[label]().text, label
        expected = STALE if label == "edited" else fresh[label]().runs[0].text
        assert outcome(lambda: runs[label].text) == expected, label
    assert [row.cells[0].text for row in table.rows] == [row.cells[0].text for row in doc.tables[0].rows]
    assert cell.text == doc.tables[0].cell(1, 1).text
    assert section.page_width == doc.section(1).page_width
    assert header.text == doc.section(0).header.paragraphs[0].text
    # Kept handles still edit their own element, and the edited paragraph takes further edits.
    runs["after"].text = "after, edited"
    assert doc.paragraphs[EDITED + 1].runs[0].text == "after, edited"
    runs["cell"].text = "cell, edited"
    assert doc.tables[0].cell(1, 1).paragraphs[0].text.startswith("cell, edited")
    paragraphs["edited"].add_run(" again")
    assert doc.paragraphs[EDITED].text.endswith(" again")
    assert texts(reopened(doc)) == texts(doc)
    assert MARKERS[call] in body_xml(doc)


REFUSED = {
    "Document.set_story_text": lambda doc, it: doc.set_story_text(it, "a\x01b"),
    "Document.insert_legacy_form_field": lambda doc, it: doc.insert_legacy_form_field(
        oxdocx.StoryRunPosition(item=it, run_index=99), LEGACY),
    "Document.add_story_permission_range past the runs": lambda doc, it: doc.add_story_permission_range(
        one_paragraph_range(it, 0, 99), editor="Clerk"),
    "Document.add_story_permission_range backwards": lambda doc, it: doc.add_story_permission_range(
        one_paragraph_range(it, 2, 1), editor="Clerk"),
    "Document.add_story_permission_range without an editor": lambda doc, it: doc.add_story_permission_range(
        one_paragraph_range(it)),
}


@pytest.mark.parametrize("call", list(REFUSED))
def test_a_refused_story_paragraph_call_leaves_the_bytes_and_every_handle_unchanged(body, call):
    doc = oxdocx.Document.from_bytes(body)
    paragraph = doc.paragraphs[EDITED]
    handles = (paragraph, list(paragraph.runs), doc.paragraphs[EDITED + 1].runs[0])
    before = doc.to_bytes()
    with pytest.raises((oxdocx.DocxError, ValueError)):
        REFUSED[call](doc, item(doc, paragraph))
    check_unchanged(doc, before, handles)


@pytest.mark.skipif(not debug_build(), reason="a debug build fails the publication on request")
@pytest.mark.parametrize("failure", ["error", "panic"])
@pytest.mark.parametrize("call", list(CALLS))
def test_a_story_paragraph_publication_that_fails_after_the_swap_puts_the_paragraph_back(body, call, failure,
                                                                                           monkeypatch):
    doc = oxdocx.Document.from_bytes(body)
    paragraph = doc.paragraphs[EDITED]
    handles = (paragraph, list(paragraph.runs), doc.paragraphs[EDITED + 1].runs[0])
    before = doc.to_bytes()
    it = item(doc, paragraph)
    monkeypatch.setenv("OXOFFICE_DEBUG_FAIL_PARAGRAPH_SPLICE", failure)
    with pytest.raises(BaseException) as raised:
        CALLS[call](doc, it)
    assert "on request" in str(raised.value)
    monkeypatch.delenv("OXOFFICE_DEBUG_FAIL_PARAGRAPH_SPLICE")
    check_unchanged(doc, before, handles)


def test_permission_ranges_take_the_smallest_free_id_and_a_refused_one_gives_it_back(body):
    doc = oxdocx.Document.from_bytes(body)
    first = doc.add_story_permission_range(one_paragraph_range(item(doc, doc.paragraphs[EDITED])), editor="Clerk")
    with pytest.raises((oxdocx.DocxError, ValueError)):
        doc.add_story_permission_range(one_paragraph_range(item(doc, doc.paragraphs[EDITED + 1]), 0, 99),
                                       editor="Clerk")
    second = doc.add_story_permission_range(one_paragraph_range(item(doc, doc.paragraphs[EDITED + 1])),
                                            group="everyone")
    assert (first, second) == (0, 1)
    again = reopened(doc)
    third = again.add_story_permission_range(one_paragraph_range(item(again, again.paragraphs[EDITED + 2])),
                                             editor="Clerk")
    assert third == 2


@pytest.mark.parametrize("call", list(CALLS))
def test_a_story_paragraph_call_on_a_header_or_a_cell_reads_the_same_after_a_reopen(body, call):
    doc = oxdocx.Document.from_bytes(body)
    targets = [it for it in doc.story_items if it.kind == "paragraph" and it.story.kind in ("header", "table_cell")]
    header = next(it for it in targets if it.story.kind == "header")
    cell = next(it for it in targets if it.story.kind == "table_cell")
    for it in (header, cell):
        # A fresh item: the first call may retire the second's handle.
        fresh = next(candidate for candidate in doc.story_items
                     if candidate.story.kind == it.story.kind and candidate.kind == "paragraph"
                     and candidate.text == it.text)
        CALLS[call](doc, fresh)
    assert texts(reopened(doc)) == texts(doc)
