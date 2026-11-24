"""docx: an edit of a comment's own story (its resolved flag) changes no element a handle names, so it keeps every
handle: a handle taken before the edit reads and edits the same element after it. The edit is made in place, without
saving the document: an edit that changes nothing leaves the document as it was, byte for byte."""
import pytest

import oxoffice.docx as oxdocx
from builders import edit_cost_docx
from test_docx_block_handles import outcome


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
