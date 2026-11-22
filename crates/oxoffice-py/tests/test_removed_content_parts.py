"""Content removed through Python leaves the pictures and links only it showed out of the saved file."""

import io
import zipfile

import oxoffice.docx as oxdocx

from builders import noise_png


def media(doc):
    return [name for name in zipfile.ZipFile(io.BytesIO(doc.to_bytes())).namelist() if name.startswith("word/media/")]


def test_a_popped_picture_leaves_the_saved_file_and_comes_back_when_pasted():
    doc = oxdocx.Document()
    doc.add_paragraph("kept")
    doc.add_picture(noise_png(4, 1), "gone.png")
    doc = oxdocx.Document.from_bytes(doc.to_bytes())
    assert len(media(doc)) == 1
    picture = next(item for item in doc.story_items if item.kind == "paragraph" and not item.text)
    fragment = doc.pop_content(picture)
    assert media(doc) == []
    body = next(story for story in doc.stories if story.kind == "body")
    doc.insert_content(body, fragment)
    assert len(media(doc)) == 1
