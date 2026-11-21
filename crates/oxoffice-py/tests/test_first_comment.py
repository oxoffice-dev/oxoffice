"""The first comment of a new document, which has no comments part yet, through every Python route: a story range
over a body paragraph, a table cell or a header (Document.add_comment with a StoryRunRange), a run range
(Document.add_comment with a RunRange), a text match (Document.add_comment_on_text), and a reply on it. Each reads
back after a save with its text, and its mark carries the comment reference style (#372)."""

import io
import re
import zipfile

import pytest

import oxoffice.docx as oxdocx


def new_document():
    """A new document: "Body" in the body, a 1 x 1 table holding "Cell", and a header "Header words"."""
    doc = oxdocx.Document()
    doc.add_paragraph("Body")
    doc.add_table(1, 1).cell(0, 0).text = "Cell"
    doc.set_header("Header words")
    return doc


def story_run(doc, kind):
    """The first run of the first paragraph of the first story of this kind."""
    item = next(item for item in doc.story_items if item.story.kind == kind and item.kind == "paragraph")
    return oxdocx.StoryRunRange(start=oxdocx.StoryRunPosition(item=item, run_index=0),
                                end=oxdocx.StoryRunPosition(item=item, run_index=1))


def reference_style(part, comment_id):
    """The character style of the run that holds the reference of comment_id in this saved part."""
    run = re.search(r'<w:r>(?:(?!</w:r>).)*<w:commentReference w:id="%d"/>' % comment_id, part, re.S)
    assert run, part
    style = re.search(r'<w:rStyle w:val="([^"]+)"/>', run.group(0))
    return style and style.group(1)


ROUTES = {
    "story range on a body paragraph": ("word/document.xml",
                                        lambda doc: doc.add_comment(story_run(doc, "body"), author="Ada", text="Note")),
    "story range in a table cell": ("word/document.xml",
                                    lambda doc: doc.add_comment(story_run(doc, "table_cell"), author="Ada",
                                                                text="Note")),
    "story range in a header": ("word/header1.xml",
                                lambda doc: doc.add_comment(story_run(doc, "header"), author="Ada", text="Note")),
    "run range": ("word/document.xml",
                  lambda doc: doc.add_comment(oxdocx.RunRange(start=oxdocx.RunPosition(body_index=0, run_index=0),
                                                              end=oxdocx.RunPosition(body_index=0, run_index=1)),
                                              author="Ada", text="Note")),
    "text": ("word/document.xml", lambda doc: doc.add_comment_on_text("Body", author="Ada", text="Note")),
}


@pytest.mark.parametrize("route", ROUTES)
def test_the_first_comment_of_a_new_document_and_a_reply_on_it(route):
    part, add = ROUTES[route]
    doc = new_document()
    assert not doc.comments
    root = add(doc)
    reply = doc.reply_to(root, author="Ben", text="Reply")
    data = doc.to_bytes()
    saved = oxdocx.Document.from_bytes(data)
    by_id = {comment.id: comment for comment in saved.comments}
    assert by_id[root].text == "Note"
    assert by_id[reply].text == "Reply"
    assert by_id[reply].parent_id == root
    xml = zipfile.ZipFile(io.BytesIO(data)).read(part).decode()
    assert reference_style(xml, root) == "CommentReference"
    assert reference_style(xml, reply) == "CommentReference"
