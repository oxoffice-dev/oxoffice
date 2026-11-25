"""docx: header, footer, watermark and section story edits run on the live document and publish the story parts they
touch. The body handles, and the handles of a header the edit leaves, keep working; the handles into a rewritten
header or footer part retire; a section that shows another story than before retires the handles taken through it;
the page colour touches no story. A refused edit, and one that changes nothing, leave the document and every handle as
they were."""
import io
import posixpath
import zipfile
from xml.etree import ElementTree

import pytest

import oxoffice.docx as oxdocx
from builders import edit_cost_docx, noise_png
from test_docx_block_handles import STALE, outcome

EMU = 914400


@pytest.fixture(scope="module")
def saved(tmp_path_factory):
    """Two sections: the first one has a header, which the second one follows; no footer. A body with tables."""
    return edit_cost_docx(tmp_path_factory.mktemp("stories") / "plain.docx", 12)


def body_handles(doc):
    """A handle of each body kind, each with how to read it and what it reads now. A section whose properties change
    (a header or footer reference) is another element, as is the paragraph that holds them: their handles retire."""
    table = doc.tables[0]
    taken = {
        "paragraph": (doc.paragraphs[3], lambda h: h.text),
        "run": (doc.paragraphs[3].runs[2], lambda h: h.text),
        "table": (table, lambda h: h.cell(1, 1).text),
        "cell": (table.cell(0, 1), lambda h: h.text),
        "cell paragraph": (table.cell(1, 0).paragraphs[0], lambda h: h.text),
    }
    return {name: (handle, read, read(handle)) for name, (handle, read) in taken.items()}


def section_handles(doc):
    return {f"section {index}": (doc.section(index), lambda h: h.page_width, doc.section(index).page_width)
            for index in (0, 1)}


def reads(taken):
    return {name: outcome(lambda: read(handle)) for name, (handle, read, _) in taken.items()}


def before(taken):
    return {name: value for name, (_, _, value) in taken.items()}


def header_texts(doc, section=0):
    return [p.text for p in doc.section(section).header.paragraphs]


def part(data, suffix):
    """The bytes of the parts whose name ends with `suffix`, joined."""
    with zipfile.ZipFile(io.BytesIO(data)) as package:
        return b"".join(package.read(name) for name in package.namelist() if name.endswith(suffix))


def headers(data):
    with zipfile.ZipFile(io.BytesIO(data)) as package:
        return b"".join(package.read(name) for name in package.namelist() if name.startswith("word/header"))


IMAGE_RELATIONSHIP = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image"


def header_images(data):
    """The bytes of each media part a header relationship of the image type names, in the saved package."""
    images = []
    with zipfile.ZipFile(io.BytesIO(data)) as package:
        names = set(package.namelist())
        for name in sorted(names):
            if not (name.startswith("word/_rels/header") and name.endswith(".xml.rels")):
                continue
            for relationship in ElementTree.fromstring(package.read(name)):
                if relationship.get("Type") != IMAGE_RELATIONSHIP or relationship.get("TargetMode") == "External":
                    continue
                # A target is relative to the header's folder, or absolute from the package root.
                target = posixpath.normpath(posixpath.join("/word", relationship.get("Target"))).lstrip("/")
                assert target.startswith("word/media/") and target in names, target
                images.append(package.read(target))
    return images


def the_body_handles_still_edit(doc, taken):
    """The kept body handles edit their element, and the save holds the edits."""
    taken["paragraph"][0].text = "edited paragraph"
    taken["cell"][0].text = "edited cell"
    again = oxdocx.Document.from_bytes(doc.to_bytes())
    assert again.paragraphs[3].text == "edited paragraph"
    assert again.tables[0].cell(0, 1).text == "edited cell"
    return again


def test_setting_the_header_retires_its_handles_alone(saved):
    doc = oxdocx.Document.open(saved)
    taken = body_handles(doc) | section_handles(doc)
    header = doc.section(0).header
    (inside,) = header.paragraphs
    doc.set_header("new header")
    assert reads(taken) == before(taken)
    assert outcome(lambda: inside.text) == STALE
    # The header handle reads its section's header anew.
    assert [p.text for p in header.paragraphs] == ["new header"]
    again = the_body_handles_still_edit(doc, taken)
    assert header_texts(again) == ["new header"]
    assert again.section(1).header.is_linked_to_previous


def test_setting_a_footer_where_there_is_none_keeps_the_body_and_header_handles(saved):
    doc = oxdocx.Document.open(saved)
    taken = body_handles(doc)
    doc.set_footer("new footer")
    assert reads(taken) == before(taken)
    # The header and its paragraphs read on.
    assert header_texts(doc) == ["header"]
    again = the_body_handles_still_edit(doc, taken)
    assert [p.text for p in again.section(0).footer.paragraphs] == ["new footer"]
    assert header_texts(again) == ["header"]


@pytest.mark.parametrize("kind", ["paragraph", "table"])
def test_adding_a_block_to_a_header_keeps_the_handles_outside_it(saved, kind):
    doc = oxdocx.Document.open(saved)
    taken = body_handles(doc) | section_handles(doc)
    header = doc.section(0).header
    (inside,) = header.paragraphs
    added = header.add_paragraph("more") if kind == "paragraph" else header.add_table(1, 2)
    assert reads(taken) == before(taken)
    assert outcome(lambda: inside.text) == STALE
    if kind == "paragraph":
        assert added.text == "more"
        added.text = "edited header"
        assert header_texts(doc) == ["header", "edited header"]
    else:
        added.cell(0, 1).text = "in the header"
        assert header.tables[0].cell(0, 1).text == "in the header"
    # The header handle edits on, and a second block follows the first.
    header.add_paragraph("last")
    again = the_body_handles_still_edit(doc, taken)
    texts = header_texts(again)
    assert texts[0] == "header" and texts[-1] == "last"
    if kind == "paragraph":
        assert texts == ["header", "edited header", "last"]
    else:
        assert again.section(0).header.tables[0].cell(0, 1).text == "in the header"


def test_adding_a_paragraph_to_an_absent_footer_creates_it(saved):
    doc = oxdocx.Document.open(saved)
    taken = body_handles(doc)
    footer = doc.section(1).footer
    added = footer.add_paragraph("first footer line")
    assert reads(taken) == before(taken)
    assert added.text == "first footer line"
    assert not footer.is_linked_to_previous
    again = the_body_handles_still_edit(doc, taken)
    assert [p.text for p in again.section(1).footer.paragraphs][-1] == "first footer line"


def test_unlinking_a_header_retires_the_handles_taken_through_that_section_alone(saved):
    doc = oxdocx.Document.open(saved)
    taken = body_handles(doc)
    first = doc.section(0).header.paragraphs[0]
    taken |= {"section 0": section_handles(doc)["section 0"]}
    second = doc.section(1).header
    through_second = second.paragraphs[0]
    second.is_linked_to_previous = False
    assert reads(taken) == before(taken)
    # The first section shows the same header: its handles stay.
    assert first.text == "header"
    # The second section shows another header now.
    assert outcome(lambda: through_second.text) == STALE
    assert not second.is_linked_to_previous
    second.paragraphs[0].text = "own header"
    assert first.text == "header"
    again = the_body_handles_still_edit(doc, taken)
    assert header_texts(again, 0) == ["header"]
    assert header_texts(again, 1) == ["own header"]
    # Linking it back keeps the first section's handles too.
    second.is_linked_to_previous = True
    assert first.text == "header"
    assert header_texts(doc, 1) == ["header"]


def test_setting_the_link_it_already_has_keeps_every_handle(saved):
    doc = oxdocx.Document.open(saved)
    taken = body_handles(doc) | section_handles(doc)
    first = doc.section(0).header.paragraphs[0]
    unchanged = doc.to_bytes()
    doc.section(1).header.is_linked_to_previous = True
    doc.section(0).header.is_linked_to_previous = False
    assert reads(taken) == before(taken)
    assert first.text == "header"
    assert doc.to_bytes() == unchanged


def test_section_stories_keep_the_body_handles(saved):
    doc = oxdocx.Document.open(saved)
    taken = body_handles(doc)
    first = doc.section(0).header.paragraphs[0]
    made = doc.create_section_story(1, "footer", "default")
    assert made.kind == "footer"
    assert reads(taken) == before(taken)
    assert first.text == "header"
    unlinked = doc.unlink_section_story(1, "header", "default")
    assert unlinked.kind == "header"
    assert reads(taken) == before(taken)
    assert first.text == "header"
    header = next(story for story in doc.stories if story.kind == "header"
                  and story.part_name != unlinked.part_name)
    linked = doc.link_section_story(1, "header", "default", header)
    assert linked.part_name == header.part_name
    assert reads(taken) == before(taken)
    assert first.text == "header"
    again = the_body_handles_still_edit(doc, taken)
    assert header_texts(again, 1) == ["header"]
    assert len([story for story in again.stories if story.kind == "header"]) == 1
    assert len([story for story in again.stories if story.kind == "footer"]) == 1


def test_creating_a_story_the_section_has_keeps_every_handle(saved):
    doc = oxdocx.Document.open(saved)
    taken = body_handles(doc) | section_handles(doc)
    first = doc.section(0).header.paragraphs[0]
    unchanged = doc.to_bytes()
    story = doc.create_section_story(0, "header", "default")
    assert story.kind == "header"
    assert reads(taken) == before(taken)
    assert first.text == "header"
    assert doc.to_bytes() == unchanged


def test_watermarks_of_the_document_keep_the_body_handles(saved):
    doc = oxdocx.Document.open(saved)
    taken = body_handles(doc)
    doc.set_text_watermark("DRAFT")
    assert reads(taken) == before(taken)
    assert b"DRAFT" in headers(doc.to_bytes())
    picture = noise_png(8, 1)
    assert picture not in header_images(doc.to_bytes())
    doc.set_image_watermark(picture, "mark.png", EMU, EMU)
    assert reads(taken) == before(taken)
    # The header keeps its text beside the watermark.
    assert header_texts(doc)[0] == "header"
    again = the_body_handles_still_edit(doc, taken)
    assert header_texts(again)[0] == "header"
    # The saved package holds the picture as a media part, which a header relationship of the image type names.
    assert picture in header_images(again.to_bytes())


def test_section_watermarks_keep_the_section_and_body_handles(saved):
    doc = oxdocx.Document.open(saved)
    taken = body_handles(doc)
    section = doc.section(0)
    (inside,) = section.header.paragraphs
    section.set_text_watermark("COPY", width=EMU, height=EMU, font_size=oxdocx.Pt(36))
    assert reads(taken) == before(taken)
    assert outcome(lambda: inside.text) == STALE
    assert b"COPY" in headers(doc.to_bytes())
    section.set_image_watermark(noise_png(8, 2), "mark.png", width=EMU, height=EMU)
    assert reads(taken) == before(taken)
    section.remove_watermark()
    assert reads(taken) == before(taken)
    assert b"COPY" not in headers(doc.to_bytes())
    # Removing a watermark that is not there changes nothing.
    kept = section.header.paragraphs[0]
    unchanged = doc.to_bytes()
    section.remove_watermark()
    assert kept.text == "header"
    assert doc.to_bytes() == unchanged
    again = the_body_handles_still_edit(doc, taken)
    assert header_texts(again)[0] == "header"


def test_a_watermark_on_a_section_that_follows_a_header_marks_that_header(saved):
    doc = oxdocx.Document.open(saved)
    taken = body_handles(doc) | section_handles(doc)
    first = doc.section(0).header.paragraphs[0]
    doc.section(1).set_text_watermark("LAST", width=EMU, height=EMU, font_size=oxdocx.Pt(36))
    assert reads(taken) == before(taken)
    # The second section shows the first one's header: the watermark is written there, which retires its handles.
    assert outcome(lambda: first.text) == STALE
    assert header_texts(doc, 0)[0] == "header"
    again = the_body_handles_still_edit(doc, taken)
    assert again.section(1).header.is_linked_to_previous
    assert b"LAST" in headers(again.to_bytes())


def test_the_page_colour_keeps_every_handle(saved):
    doc = oxdocx.Document.open(saved)
    taken = body_handles(doc) | section_handles(doc)
    first = doc.section(0).header.paragraphs[0]
    doc.set_page_color("FFF2CC")
    assert reads(taken) == before(taken)
    assert first.text == "header"
    again = the_body_handles_still_edit(doc, taken)
    assert b'w:color="FFF2CC"' in part(again.to_bytes(), "word/document.xml")


@pytest.mark.parametrize("refused", [
    lambda doc: doc.set_header("bad \x01 header"),
    lambda doc: doc.set_footer("bad \x01 footer"),
    lambda doc: doc.set_page_color("not a colour"),
    lambda doc: doc.set_text_watermark("bad \x01 mark"),
    lambda doc: doc.section(0).header.add_paragraph("text", style="No Such Style"),
    lambda doc: doc.unlink_section_story(1, "footer", "default"),
    lambda doc: doc.section(1).set_text_watermark("bad \x01", width=EMU, height=EMU, font_size=oxdocx.Pt(36)),
], ids=["header", "footer", "page colour", "watermark", "header style", "absent story", "section watermark"])
def test_a_refused_edit_leaves_the_document_and_every_handle(saved, refused):
    doc = oxdocx.Document.open(saved)
    taken = body_handles(doc) | section_handles(doc)
    first = doc.section(0).header.paragraphs[0]
    unchanged = doc.to_bytes()
    with pytest.raises((ValueError, KeyError, oxdocx.DocxError)):
        refused(doc)
    assert doc.to_bytes() == unchanged
    assert reads(taken) == before(taken)
    assert first.text == "header"
    the_body_handles_still_edit(doc, taken)
