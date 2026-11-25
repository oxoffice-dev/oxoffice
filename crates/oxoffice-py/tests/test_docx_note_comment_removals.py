"""docx: removing a note or a comment thread edits the paragraphs that hold its references or marks and the note or
comment part in place: the removed note's handles retire, an edited paragraph keeps its handle while its runs are born
again, and every other handle, a handle into a note before or after the removed one included, keeps reading and
editing its element. A refused or unknown removal leaves the document as it was."""

import pytest

import oxoffice.docx as oxdocx
from builders import edit_cost_docx
from test_docx_block_handles import STALE, outcome
from test_docx_comment_edits import before, body_range, debug_build, handles_taken, reads


@pytest.fixture(scope="module")
def saved(tmp_path_factory):
    """The edit-cost document with a footnote on paragraph 2 and an endnote on paragraph 4."""
    path = edit_cost_docx(tmp_path_factory.mktemp("removals") / "plain.docx", 12)
    doc = oxdocx.Document.open(path)
    doc.add_footnote(doc.paragraphs[2], "other footnote")
    doc.add_endnote(doc.paragraphs[4], "other endnote")
    out = path.with_name("saved.docx")
    out.write_bytes(doc.to_bytes())
    return out


def add_to(doc, kind):
    return doc.add_footnote if kind == "footnote" else doc.add_endnote


def remove_from(doc, kind):
    return doc.remove_footnote if kind == "footnote" else doc.remove_endnote


def note_texts(doc, kind):
    return [p.text.strip() for p in doc.story_paragraphs if p.kind == kind and p.text.strip()]


def three_notes(doc, kind):
    """Notes on paragraphs 6, 7 and 8, after the fixture's: a handle to each note's paragraph, and the middle note's
    id."""
    add = add_to(doc, kind)
    first = add(doc.paragraphs[6], "first").paragraphs[0]
    middle = add(doc.paragraphs[7], "middle")
    last = add(doc.paragraphs[8], "last").paragraphs[0]
    return first, middle.paragraphs[0], middle.id, last


@pytest.mark.parametrize("kind", ["footnote", "endnote"])
def test_removing_a_note_keeps_the_handles_of_the_notes_before_and_after_it(saved, kind):
    doc = oxdocx.Document.open(saved)
    first, middle, middle_id, last = three_notes(doc, kind)
    taken = handles_taken(doc)
    edited = doc.paragraphs[7]
    text = edited.text
    edited_run = edited.runs[1]

    remove_from(doc, kind)(middle_id)

    assert note_texts(doc, kind) == [f"other {kind}", "first", "last"]
    # The notes before and after the removed one keep their handles, which read and edit their own note.
    assert first.text.strip() == "first"
    assert last.text.strip() == "last"
    assert reads(taken) == before(taken)
    # The removed note's handle retires; the edited paragraph keeps its handle, its runs are born again.
    assert outcome(lambda: middle.text) == STALE
    assert edited.text == text
    assert f"w:{kind}Reference" not in edited.xml
    assert outcome(lambda: edited_run.text) == STALE
    last.text = "edited last"
    first.text = "edited first"
    taken["run"][0].text = "edited run"
    again = oxdocx.Document.from_bytes(doc.to_bytes())
    assert note_texts(again, kind) == [f"other {kind}", "edited first", "edited last"]
    assert again.paragraphs[3].runs[2].text == "edited run"


@pytest.mark.parametrize("kind", ["footnote", "endnote"])
def test_deleting_a_note_keeps_every_other_handle(saved, kind):
    doc = oxdocx.Document.open(saved)
    first = add_to(doc, kind)(doc.paragraphs[6], "first").paragraphs[0]
    note = add_to(doc, kind)(doc.paragraphs[8], "deleted")
    inside = note.paragraphs[0]
    taken = handles_taken(doc)
    edited = doc.paragraphs[8]
    edited_run = edited.runs[0]

    note.delete()

    assert note_texts(doc, kind) == [f"other {kind}", "first"]
    assert first.text.strip() == "first"
    assert reads(taken) == before(taken)
    assert outcome(lambda: inside.text) == STALE
    assert outcome(lambda: note.paragraphs) == STALE
    assert f"w:{kind}Reference" not in edited.xml
    assert outcome(lambda: edited_run.text) == STALE
    first.text = "edited first"
    again = oxdocx.Document.from_bytes(doc.to_bytes())
    assert note_texts(again, kind) == [f"other {kind}", "edited first"]


@pytest.mark.parametrize("kind", ["footnote", "endnote"])
def test_removing_an_unknown_note_leaves_the_document_and_every_handle_as_they_were(saved, kind):
    doc = oxdocx.Document.open(saved)
    first, middle, _, last = three_notes(doc, kind)
    taken = handles_taken(doc)
    data = doc.to_bytes()

    with pytest.raises(oxdocx.DocxError, match="does not exist"):
        remove_from(doc, kind)(9999)

    assert doc.to_bytes() == data
    assert reads(taken) == before(taken)
    assert [p.text.strip() for p in (first, middle, last)] == ["first", "middle", "last"]


@pytest.mark.parametrize("kind", ["footnote", "endnote"])
def test_a_refused_note_removal_leaves_the_document_and_every_handle_as_they_were(saved, kind):
    doc = oxdocx.Document.open(saved)
    first, middle, middle_id, last = three_notes(doc, kind)
    taken = handles_taken(doc)
    data = doc.to_bytes()

    with pytest.raises(TypeError, match="non-bool integer"):
        remove_from(doc, kind)(str(middle_id))

    assert doc.to_bytes() == data
    assert reads(taken) == before(taken)
    assert [p.text.strip() for p in (first, middle, last)] == ["first", "middle", "last"]


@pytest.mark.skipif(not debug_build(), reason="a debug build fails the publication on request")
@pytest.mark.parametrize("failure", ["error", "panic"])
@pytest.mark.parametrize("kind", ["footnote", "endnote"])
def test_a_note_removal_whose_publication_fails_leaves_the_document_and_every_handle_as_they_were(
        saved, kind, failure, monkeypatch):
    doc = oxdocx.Document.open(saved)
    first, middle, middle_id, last = three_notes(doc, kind)
    taken = handles_taken(doc)
    edited_run = doc.paragraphs[7].runs[1]
    data = doc.to_bytes()

    # Debug builds fail the publication of the paragraph and the note on request, before its one commit.
    monkeypatch.setenv("OXOFFICE_DEBUG_FAIL_PARAGRAPH_SPLICE", failure)
    with pytest.raises(BaseException) as raised:
        remove_from(doc, kind)(middle_id)
    monkeypatch.delenv("OXOFFICE_DEBUG_FAIL_PARAGRAPH_SPLICE")

    assert "on request" in str(raised.value)
    assert doc.to_bytes() == data
    assert reads(taken) == before(taken)
    assert [p.text.strip() for p in (first, middle, last)] == ["first", "middle", "last"]
    assert edited_run.text == "run 1 of paragraph 7 "
    remove_from(doc, kind)(middle_id)
    assert note_texts(doc, kind) == [f"other {kind}", "first", "last"]
    assert last.text.strip() == "last"


def comment_texts(doc):
    return [comment.text for comment in doc.comments]


def test_removing_a_comment_thread_over_three_paragraphs_keeps_every_other_handle(saved):
    doc = oxdocx.Document.open(saved)
    thread = doc.add_comment(body_range(doc, 0, 1, last=8), author="Reviewer", text="three paragraphs")
    doc.reply_to(thread, author="Writer", text="a reply")
    later = doc.add_comment(oxdocx.RunRange(
        start=oxdocx.RunPosition(body_index=doc.find_content_index(doc.paragraphs[10]), run_index=0),
        end=oxdocx.RunPosition(body_index=doc.find_content_index(doc.paragraphs[10]), run_index=1)),
        author="Reviewer", text="a later remark")
    # Comments added since the open are not written yet: the removal of one takes the whole route.
    doc = oxdocx.Document.from_bytes(doc.to_bytes())
    texts = comment_texts(doc)
    taken = handles_taken(doc)
    spanned = [doc.paragraphs[index] for index in (6, 7, 8)]
    spanned_texts = [paragraph.text for paragraph in spanned]
    later_paragraph = doc.paragraphs[10]
    marked_run = spanned[0].runs[0]

    assert doc.remove_comment(thread) is True

    assert comment_texts(doc) == [text for text in texts if text not in ("three paragraphs", "a reply")]
    assert [comment.text for comment in doc.comments if comment.id == later] == ["a later remark"]
    assert reads(taken) == before(taken)
    # The paragraphs the thread spanned keep their handles; a run of a paragraph that held a mark is born again.
    assert [paragraph.text for paragraph in spanned] == spanned_texts
    assert all("w:commentRangeStart" not in paragraph.xml for paragraph in spanned)
    assert all("w:commentReference" not in paragraph.xml for paragraph in spanned)
    assert outcome(lambda: marked_run.text) == STALE
    assert later_paragraph.text.startswith("run 0 of paragraph 10")
    spanned[1].text = "edited middle paragraph"
    again = oxdocx.Document.from_bytes(doc.to_bytes())
    assert comment_texts(again) == comment_texts(doc)
    assert again.paragraphs[7].text == "edited middle paragraph"
    (saved_later,) = [comment for comment in again.comments if comment.id == later]
    assert saved_later.anchor_text.startswith("run 0 of paragraph 10")
    # The comment after the removed thread is removed in place in turn.
    assert again.remove_comment(later) is True
    assert "a later remark" not in comment_texts(again)


def test_removing_an_unknown_comment_leaves_the_document_and_every_handle_as_they_were(saved):
    doc = oxdocx.Document.open(saved)
    taken = handles_taken(doc)
    data = doc.to_bytes()

    assert doc.remove_comment(9999) is False

    assert doc.to_bytes() == data
    assert reads(taken) == before(taken)


def test_a_refused_comment_removal_leaves_the_document_and_every_handle_as_they_were(saved):
    doc = oxdocx.Document.open(saved)
    taken = handles_taken(doc)
    data = doc.to_bytes()

    with pytest.raises(TypeError):
        doc.remove_comment("1")

    assert doc.to_bytes() == data
    assert reads(taken) == before(taken)
