"""docx: an edit of a comment's own story (its resolved flag) changes no element a handle names, so it keeps every
handle: a handle taken before the edit reads and edits the same element after it. The edit is made in place, without
saving the document: an edit that changes nothing leaves the document as it was, byte for byte."""
import io
import re
import zipfile

import pytest

import oxoffice.docx as oxdocx
from builders import edit_cost_docx
from test_docx_block_handles import STALE, outcome


def debug_build():
    import oxoffice.docx._native as native

    return native._debug_build


@pytest.fixture(scope="module")
def saved(tmp_path_factory):
    """A body whose paragraphs, runs and rows carry Word's identifiers, two sections, the first with a header, and a
    comment on the fifth paragraph."""
    path = edit_cost_docx(tmp_path_factory.mktemp("comments") / "plain.docx", 12)
    doc = oxdocx.Document.open(path)
    doc.add_comment_on_text(doc.paragraphs[5].runs[0].text, author="Reviewer", text="a remark")
    out = path.with_name("saved.docx")
    out.write_bytes(doc.to_bytes())
    return out


def handles_taken(doc):
    """A handle of each kind the binding tracks, each with how to read it and what it reads now."""
    table = doc.tables[0]
    taken = {
        "paragraph": (doc.paragraphs[3], lambda h: h.text),
        "commented paragraph": (doc.paragraphs[5], lambda h: h.text),
        "run": (doc.paragraphs[3].runs[2], lambda h: h.text),
        "table": (table, lambda h: h.cell(1, 1).text),
        "cell": (table.cell(0, 1), lambda h: h.text),
        "cell paragraph": (table.cell(1, 0).paragraphs[0], lambda h: h.text),
        "header paragraph": (doc.section(0).header.paragraphs[0], lambda h: h.text),
        "section": (doc.section(0), lambda h: h.page_width),
    }
    return {name: (handle, read, read(handle)) for name, (handle, read) in taken.items()}


def reads(taken):
    return {name: outcome(lambda: read(handle)) for name, (handle, read, _) in taken.items()}


def before(taken):
    return {name: value for name, (_, _, value) in taken.items()}


def the_comment(doc):
    (comment,) = [comment for comment in doc.comments if comment.text == "a remark"]
    return comment


@pytest.mark.parametrize("resolved", [True, False])
def test_resolving_a_comment_keeps_every_handle(saved, resolved):
    doc = oxdocx.Document.open(saved)
    taken = handles_taken(doc)
    assert doc.resolve_comment(the_comment(doc).id, resolved=resolved)
    assert the_comment(doc).resolved is resolved
    assert reads(taken) == before(taken)
    # The handles still edit their element.
    taken["paragraph"][0].text = "edited paragraph"
    taken["cell"][0].text = "edited cell"
    assert doc.paragraphs[3].text == "edited paragraph"
    assert doc.tables[0].cell(0, 1).text == "edited cell"
    again = oxdocx.Document.from_bytes(doc.to_bytes())
    assert the_comment(again).resolved is resolved
    assert again.paragraphs[3].text == "edited paragraph"


def test_resolving_twice_reads_the_same_document(saved):
    doc = oxdocx.Document.open(saved)
    comment = the_comment(doc).id
    doc.resolve_comment(comment)
    once = doc.to_bytes()
    taken = handles_taken(doc)
    doc.resolve_comment(comment)
    assert doc.to_bytes() == once
    assert reads(taken) == before(taken)


def test_resolving_an_unknown_comment_leaves_the_document_as_it_was(saved):
    doc = oxdocx.Document.open(saved)
    taken = handles_taken(doc)
    unchanged = doc.to_bytes()
    assert doc.resolve_comment(max(comment.id for comment in doc.comments) + 100) is False
    assert doc.to_bytes() == unchanged
    assert reads(taken) == before(taken)


def test_resolving_a_thread_keeps_every_handle_and_marks_its_root(saved):
    doc = oxdocx.Document.open(saved)
    parent = the_comment(doc).id
    reply = doc.reply_to(parent, author="Ben", text="agreed")
    doc = oxdocx.Document.from_bytes(doc.to_bytes())
    taken = handles_taken(doc)
    assert doc.resolve_comment(parent)
    assert reads(taken) == before(taken)
    again = oxdocx.Document.from_bytes(doc.to_bytes())
    by_id = {comment.id: comment for comment in again.comments}
    assert by_id[parent].resolved is True
    assert by_id[reply].parent_id == parent


def without_comments_extended(source):
    """`source` as a producer without the comments extension part writes it (Word 2007, LibreOffice)."""
    out = io.BytesIO()
    with zipfile.ZipFile(source) as zin, zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as zout:
        for item in zin.infolist():
            if item.filename == "word/commentsExtended.xml":
                continue
            data = zin.read(item.filename)
            if item.filename == "[Content_Types].xml":
                data = re.sub(rb'<Override PartName="/word/commentsExtended.xml"[^>]*/>', b"", data)
            if item.filename == "word/_rels/document.xml.rels":
                data = re.sub(rb'<Relationship [^>]*Target="commentsExtended.xml"[^>]*/>', b"", data)
            zout.writestr(item, data)
    return out.getvalue()


def test_resolving_a_comment_adds_the_extension_part_and_keeps_every_handle(saved):
    source = without_comments_extended(saved)
    assert b"commentsExtended" not in zipfile.ZipFile(io.BytesIO(source)).read("[Content_Types].xml")
    doc = oxdocx.Document.from_bytes(source)
    taken = handles_taken(doc)
    assert doc.resolve_comment(the_comment(doc).id)
    assert reads(taken) == before(taken)
    written = doc.to_bytes()
    assert "word/commentsExtended.xml" in zipfile.ZipFile(io.BytesIO(written)).namelist()
    assert the_comment(oxdocx.Document.from_bytes(written)).resolved is True


def body_range(doc, start, end, last=None):
    """The runs `start` to `end` of paragraph 6, or to run `end` of paragraph `last`."""
    first = doc.find_content_index(doc.paragraphs[6])
    to = first if last is None else doc.find_content_index(doc.paragraphs[last])
    return oxdocx.RunRange(start=oxdocx.RunPosition(body_index=first, run_index=start),
                           end=oxdocx.RunPosition(body_index=to, run_index=end))


@pytest.mark.parametrize("end", [1, 4])
def test_a_comment_on_one_body_paragraph_keeps_every_other_handle(saved, end):
    doc = oxdocx.Document.open(saved)
    taken = handles_taken(doc)
    edited = doc.paragraphs[6]
    text = edited.text
    edited_run = edited.runs[0]

    added = doc.add_comment(body_range(doc, 0, end), author="Reviewer", text="new remark")

    (comment,) = [comment for comment in doc.comments if comment.id == added]
    assert comment.text == "new remark"
    assert the_comment(doc).text == "a remark"
    # The paragraph and the comment edited alone: every handle outside them reads what it read.
    assert reads(taken) == before(taken)
    # The edited paragraph keeps its handle; its runs are born again, as any edit of the paragraph makes them.
    assert edited.text == text
    assert outcome(lambda: edited_run.text) == STALE
    assert "w:commentRangeStart" in edited.xml and "w:commentReference" in edited.xml
    taken["run"][0].text = "edited run"
    edited.runs[0].text = "edited commented run "
    again = oxdocx.Document.from_bytes(doc.to_bytes())
    assert again.paragraphs[3].runs[2].text == "edited run"
    (saved_comment,) = [comment for comment in again.comments if comment.id == added]
    assert saved_comment.text == "new remark"
    assert saved_comment.anchor_text.startswith("edited commented run ")


def test_a_refused_comment_leaves_the_document_and_every_handle_as_they_were(saved):
    doc = oxdocx.Document.open(saved)
    taken = handles_taken(doc)
    edited_run = doc.paragraphs[6].runs[0]
    data = doc.to_bytes()

    with pytest.raises(oxdocx.DocxError, match="exceeds paragraph run count"):
        doc.add_comment(body_range(doc, 0, 99), author="Reviewer", text="new remark")

    assert doc.to_bytes() == data
    assert reads(taken) == before(taken)
    assert edited_run.text == "run 0 of paragraph 6 "


def test_a_comment_over_two_paragraphs_is_added_and_saved(saved):
    doc = oxdocx.Document.open(saved)

    added = doc.add_comment(body_range(doc, 0, 1, last=7), author="Reviewer", text="two paragraphs")

    assert "w:commentRangeStart" in doc.paragraphs[6].xml
    assert "w:commentRangeEnd" in doc.paragraphs[7].xml
    again = oxdocx.Document.from_bytes(doc.to_bytes())
    (comment,) = [comment for comment in again.comments if comment.id == added]
    assert comment.text == "two paragraphs"
    assert comment.anchor_text.startswith("run 0 of paragraph 6 ")


def test_a_reply_keeps_every_handle_outside_its_paragraph_and_its_story(saved):
    doc = oxdocx.Document.open(saved)
    taken = handles_taken(doc)
    parent = the_comment(doc)
    edited = doc.paragraphs[5]
    text = edited.text
    edited_run = edited.runs[0]

    reply = doc.reply_to(parent.id, author="Ben", text="agreed", date="2026-11-25T10:00:00Z")

    # The commented paragraph and the comments edited alone: every handle outside them reads what it read, the
    # parent comment's too.
    assert reads(taken) == before(taken)
    assert parent.text == "a remark"
    assert parent.id == the_comment(doc).id
    (written,) = [comment for comment in doc.comments if comment.id == reply]
    assert (written.text, written.parent_id) == ("agreed", parent.id)
    # The edited paragraph keeps its handle; its runs are born again.
    assert edited.text == text
    assert outcome(lambda: edited_run.text) == STALE
    assert f'w:commentReference w:id="{reply}"' in edited.xml
    taken["run"][0].text = "edited run"
    taken["cell"][0].text = "edited cell"
    again = oxdocx.Document.from_bytes(doc.to_bytes())
    assert again.paragraphs[3].runs[2].text == "edited run"
    assert again.tables[0].cell(0, 1).text == "edited cell"
    by_id = {comment.id: comment for comment in again.comments}
    assert by_id[reply].parent_id == parent.id
    assert by_id[reply].anchor_text == by_id[parent.id].anchor_text


def test_a_reply_to_an_unknown_comment_leaves_the_document_and_every_handle_as_they_were(saved):
    doc = oxdocx.Document.open(saved)
    taken = handles_taken(doc)
    edited_run = doc.paragraphs[5].runs[0]
    data = doc.to_bytes()

    with pytest.raises(oxdocx.DocxError, match="does not exist"):
        doc.reply_to(max(comment.id for comment in doc.comments) + 100, author="Ben", text="agreed")

    assert doc.to_bytes() == data
    assert reads(taken) == before(taken)
    assert edited_run.text == doc.paragraphs[5].runs[0].text


def test_a_reply_to_a_comment_in_a_header_is_added_and_saved(saved):
    doc = oxdocx.Document.open(saved)
    header = doc.section(0).header.paragraphs[0]
    (item,) = [it for it in doc.story_items if it.story.kind == "header" and it.kind == "paragraph"
               and it.text == header.text]
    parent = doc.add_comment(oxdocx.StoryRunRange(start=oxdocx.StoryRunPosition(item=item, run_index=0),
                                                  end=oxdocx.StoryRunPosition(item=item, run_index=1)),
                             author="Reviewer", text="on the header")

    reply = doc.reply_to(parent, author="Ben", text="agreed")

    again = oxdocx.Document.from_bytes(doc.to_bytes())
    by_id = {comment.id: comment for comment in again.comments}
    assert by_id[reply].parent_id == parent
    assert by_id[reply].anchor_text == by_id[parent].anchor_text


def commented_range(doc, start, end, paragraph=5):
    """Runs start..end of body paragraph `paragraph` (5 holds the comment), as a StoryRunRange."""
    index = doc.find_content_index(doc.paragraphs[paragraph])
    (item,) = [it for it in doc.story_items if it.story.kind == "body" and it.kind == "paragraph"
               and it.direct_body_index == index]
    return oxdocx.StoryRunRange(start=oxdocx.StoryRunPosition(item=item, run_index=start),
                                end=oxdocx.StoryRunPosition(item=item, run_index=end))


def test_a_comment_moved_within_its_paragraph_keeps_every_handle_outside_it(saved):
    doc = oxdocx.Document.open(saved)
    taken = handles_taken(doc)
    comment = the_comment(doc)
    edited = doc.paragraphs[5]
    text = edited.text
    edited_run = edited.runs[0]
    moved_onto = edited.runs[2].text

    # Run 1 is the reference run: runs 2..3 are the paragraph's second text run.
    doc.move_comment(comment.id, commented_range(doc, 2, 3))

    del taken["commented paragraph"]
    assert reads(taken) == before(taken)
    assert comment.text == "a remark"
    assert the_comment(doc).anchor_text == moved_onto
    # The edited paragraph keeps its handle; its runs are born again.
    assert edited.text == text
    assert outcome(lambda: edited_run.text) == STALE
    taken["run"][0].text = "edited run"
    again = oxdocx.Document.from_bytes(doc.to_bytes())
    assert again.paragraphs[3].runs[2].text == "edited run"
    assert the_comment(again).anchor_text == moved_onto
    assert again.paragraphs[5].text == text


def test_a_comment_moved_to_another_paragraph_is_moved_and_saved(saved):
    doc = oxdocx.Document.open(saved)
    comment = the_comment(doc)

    doc.move_comment(comment.id, commented_range(doc, 0, 1, paragraph=7))

    again = oxdocx.Document.from_bytes(doc.to_bytes())
    assert the_comment(again).anchor_text == again.paragraphs[7].runs[0].text


@pytest.mark.parametrize("refused", ["unknown comment", "past the last run", "reversed"])
def test_a_refused_comment_move_leaves_the_document_and_every_handle_as_they_were(saved, refused):
    doc = oxdocx.Document.open(saved)
    taken = handles_taken(doc)
    comment = the_comment(doc)
    edited_run = doc.paragraphs[5].runs[0]
    data = doc.to_bytes()
    comment_id, start, end = {
        "unknown comment": (comment.id + 100, 2, 3),
        "past the last run": (comment.id, 0, 40),
        "reversed": (comment.id, 3, 1),
    }[refused]

    with pytest.raises(oxdocx.DocxError):
        doc.move_comment(comment_id, commented_range(doc, start, end))

    assert doc.to_bytes() == data
    assert reads(taken) == before(taken)
    assert edited_run.text == doc.paragraphs[5].runs[0].text


@pytest.mark.skipif(not debug_build(), reason="a debug build fails the publication on request")
@pytest.mark.parametrize("failure", ["error", "panic"])
def test_a_comment_whose_publication_fails_leaves_the_document_and_every_handle_as_they_were(
        saved, failure, monkeypatch):
    doc = oxdocx.Document.open(saved)
    taken = handles_taken(doc)
    edited_run = doc.paragraphs[6].runs[0]
    data = doc.to_bytes()

    # Debug builds fail the publication of the paragraph and the comment on request, before its one commit.
    monkeypatch.setenv("OXOFFICE_DEBUG_FAIL_PARAGRAPH_SPLICE", failure)
    with pytest.raises(BaseException) as raised:
        doc.add_comment(body_range(doc, 0, 1), author="Reviewer", text="new remark")
    monkeypatch.delenv("OXOFFICE_DEBUG_FAIL_PARAGRAPH_SPLICE")

    assert "on request" in str(raised.value)
    assert doc.to_bytes() == data
    assert reads(taken) == before(taken)
    assert edited_run.text == "run 0 of paragraph 6 "
    added = doc.add_comment(body_range(doc, 0, 1), author="Reviewer", text="new remark")
    assert [comment.text for comment in doc.comments if comment.id == added] == ["new remark"]
