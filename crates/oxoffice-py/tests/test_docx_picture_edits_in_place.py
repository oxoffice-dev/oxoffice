"""docx: `Run.add_picture`, `Document.set_picture_size` and `Document.set_picture_sizes_for_relationship` on pictures
of direct body paragraphs edit those blocks in place.

`Run.add_picture` and `Document.set_picture_size` keep the handles the block replacement keeps: the edited paragraph's
handle stays and its runs retire; every other paragraph, run and table keeps reading and editing its own element. A
picture handle holds the size it was read with, so a resize retires it, as the staged route does; a fresh one from
`Document.pictures()` resizes again. `Document.set_picture_sizes_for_relationship` changes no element a handle names
and keeps every handle. A refused call leaves the bytes and every handle as they were. Pictures of a table cell or a
header take the staged route and read the same after a reopen."""

import io

import docx
import pytest

import oxoffice.docx as oxdocx
from builders import edit_cost_docx, noise_png
from oxoffice.docx.shared import Inches
from test_docx_paragraph_edits_in_place import EDITED, STALE, debug_build, outcome, reopened


@pytest.fixture(scope="module")
def body(tmp_path_factory):
    path = edit_cost_docx(tmp_path_factory.mktemp("pictures") / "body.docx", 25, picture_side=20, probes=1)
    return path.read_bytes()


def handles(doc):
    """The edited paragraph, its runs, and the paragraphs and runs around it."""
    paragraph = doc.paragraphs[EDITED]
    return paragraph, list(paragraph.runs), doc.paragraphs[EDITED - 1], doc.paragraphs[EDITED + 1].runs[0]


def check_neighbours_kept(doc, kept):
    paragraph, _, before, after_run = kept
    assert paragraph.text == doc.paragraphs[EDITED].text
    assert before.text == doc.paragraphs[EDITED - 1].text
    after_run.text = "still mine"
    assert doc.paragraphs[EDITED + 1].runs[0].text == "still mine"
    assert doc.tables[0].cell(0, 0).text == "cell 0 of table 0"


def sizes(doc, relationship_id):
    return sorted((picture.width, picture.height) for picture in doc.pictures()
                  if picture.relationship_id == relationship_id and picture.location.story.kind == "body")


def test_a_run_picture_is_added_in_its_paragraph_and_keeps_every_handle_outside_it(body):
    doc = oxdocx.Document.from_bytes(body)
    kept = handles(doc)
    text = kept[0].text
    picture = kept[1][2].add_picture(noise_png(8, 1), "added.png", Inches(1), Inches(0.5))
    assert (picture.width, picture.height) == (Inches(1), Inches(0.5))
    assert picture.content_type == "image/png" and picture.bytes == noise_png(8, 1)
    # The returned picture is the document's: it resizes, and reads back.
    doc.set_picture_size(picture, Inches(2), Inches(1))
    # The paragraph stays, its runs retire, every other handle stays.
    assert outcome(lambda: kept[1][0].text) == STALE
    check_neighbours_kept(doc, kept)
    assert doc.paragraphs[EDITED].text == text
    again = reopened(doc)
    found = [p for p in again.pictures() if p.bytes == noise_png(8, 1)]
    assert [(p.width, p.height) for p in found] == [(Inches(2), Inches(1))]
    assert again.paragraphs[EDITED].text == text


def test_repeated_run_pictures_take_their_own_media_and_drawing_and_read_back(body):
    doc = oxdocx.Document.from_bytes(body)
    paragraph = doc.paragraphs[EDITED]
    first = paragraph.runs[0].add_picture(noise_png(8, 2), "a.png", Inches(1), Inches(1))
    second = paragraph.runs[0].add_picture(noise_png(8, 3), "b.png", Inches(1), Inches(1))
    third = paragraph.runs[-1].add_picture(noise_png(8, 4), "c.png")
    assert len({first.relationship_id, second.relationship_id, third.relationship_id}) == 3
    assert len({first.media_part_name, second.media_part_name, third.media_part_name}) == 3
    # The second one sits before the first: it takes the first ordinal.
    assert (second.occurrence, third.occurrence) == (0, 2)
    assert paragraph.text == doc.paragraphs[EDITED].text
    again = reopened(doc)
    added = {p.bytes: p for p in again.pictures() if p.bytes in {noise_png(8, k) for k in (2, 3, 4)}}
    assert sorted(p.occurrence for p in added.values()) == [0, 1, 2]
    ids = again.to_bytes()
    assert ids == reopened(again).to_bytes()


@pytest.mark.parametrize("call", ["no image", "one length", "negative length"])
def test_a_refused_run_picture_leaves_the_bytes_and_every_handle_unchanged(body, call):
    doc = oxdocx.Document.from_bytes(body)
    kept = handles(doc)
    before = doc.to_bytes()
    run = kept[1][0]
    with pytest.raises((oxdocx.DocxError, ValueError, TypeError)):
        if call == "no image":
            run.add_picture(b"not an image", "x.png", Inches(1), Inches(1))
        elif call == "one length":
            run.add_picture(noise_png(8, 5), "x.png", Inches(1))
        else:
            run.add_picture(noise_png(8, 5), "x.png", Inches(-1), Inches(1))
    assert doc.to_bytes() == before
    assert kept[1][0].text == doc.paragraphs[EDITED].runs[0].text
    check_neighbours_kept(doc, kept)


@pytest.mark.skipif(not debug_build(), reason="a debug build fails the publication on request")
@pytest.mark.parametrize("failure", ["error", "panic"])
def test_a_run_picture_whose_publication_fails_takes_back_its_media(body, failure, monkeypatch):
    doc = oxdocx.Document.from_bytes(body)
    kept = handles(doc)
    before = doc.to_bytes()
    monkeypatch.setenv("OXOFFICE_DEBUG_FAIL_PARAGRAPH_SPLICE", failure)
    with pytest.raises(BaseException) as raised:
        kept[1][0].add_picture(noise_png(8, 6), "x.png", Inches(1), Inches(1))
    assert "on request" in str(raised.value)
    monkeypatch.delenv("OXOFFICE_DEBUG_FAIL_PARAGRAPH_SPLICE")
    assert doc.to_bytes() == before
    assert kept[1][0].text == doc.paragraphs[EDITED].runs[0].text
    # The media name and relationship it reserved are free again.
    picture = kept[1][0].add_picture(noise_png(8, 6), "x.png", Inches(1), Inches(1))
    fresh = oxdocx.Document.from_bytes(before)
    expected = fresh.paragraphs[EDITED].runs[0].add_picture(noise_png(8, 6), "x.png", Inches(1), Inches(1))
    assert (picture.relationship_id, picture.media_part_name) == (expected.relationship_id, expected.media_part_name)


def test_a_body_picture_resize_keeps_every_handle_outside_its_paragraph_and_retires_the_picture(body):
    doc = oxdocx.Document.from_bytes(body)
    picture = doc.paragraphs[EDITED].runs[0].add_picture(noise_png(8, 7), "p.png", Inches(1), Inches(1))
    kept = handles(doc)
    read = [p for p in doc.pictures() if p.bytes == noise_png(8, 7)][0]
    doc.set_picture_size(read, Inches(3), Inches(2))
    # Both handles hold the old size: they retire, as after the staged route.
    for stale in (picture, read):
        with pytest.raises(oxdocx.StaleElementError):
            doc.set_picture_size(stale, Inches(1), Inches(1))
    assert outcome(lambda: kept[1][0].text) == STALE
    check_neighbours_kept(doc, kept)
    fresh = [p for p in doc.pictures() if p.bytes == noise_png(8, 7)][0]
    assert (fresh.width, fresh.height) == (Inches(3), Inches(2))
    # A repeated resize, and a resize to the same size, which keeps the handle.
    doc.set_picture_size(fresh, Inches(4), Inches(4))
    fresh = [p for p in doc.pictures() if p.bytes == noise_png(8, 7)][0]
    doc.set_picture_size(fresh, Inches(4), Inches(4))
    doc.set_picture_size(fresh, Inches(5), Inches(4))
    assert [(p.width, p.height) for p in reopened(doc).pictures() if p.bytes == noise_png(8, 7)] == [
        (Inches(5), Inches(4))]


def test_a_picture_from_the_builder_resizes_in_place_and_reads_back(body):
    doc = oxdocx.Document.from_bytes(body)
    pictures = [p for p in doc.pictures() if p.location.story.kind == "body"]
    paragraph = doc.paragraphs[-2]
    doc.set_picture_size(pictures[0], Inches(2), Inches(2))
    assert paragraph.text == doc.paragraphs[-2].text
    again = reopened(doc)
    assert [(p.width, p.height) for p in again.pictures() if p.bytes == pictures[0].bytes] == [(Inches(2), Inches(2))]
    assert [(p.width, p.height) for p in again.pictures() if p.bytes == pictures[1].bytes] == [
        (pictures[1].width, pictures[1].height)]


@pytest.mark.parametrize("size", [(0, 1), (1, -1)])
def test_a_refused_picture_resize_leaves_the_bytes_and_every_handle_unchanged(body, size):
    doc = oxdocx.Document.from_bytes(body)
    picture = doc.paragraphs[EDITED].runs[0].add_picture(noise_png(8, 8), "p.png", Inches(1), Inches(1))
    kept = handles(doc)
    before = doc.to_bytes()
    with pytest.raises((oxdocx.DocxError, ValueError)):
        doc.set_picture_size(picture, Inches(size[0]), Inches(size[1]))
    assert doc.to_bytes() == before
    assert kept[1][0].text == doc.paragraphs[EDITED].runs[0].text
    doc.set_picture_size(picture, Inches(2), Inches(2))
    check_neighbours_kept(doc, kept)


def test_a_stale_picture_is_refused_and_changes_nothing(body):
    doc = oxdocx.Document.from_bytes(body)
    picture = doc.paragraphs[EDITED].runs[0].add_picture(noise_png(8, 9), "p.png", Inches(1), Inches(1))
    # A relationship resize keeps every handle: this one still names the old size.
    doc.set_picture_sizes_for_relationship(picture.relationship_id, Inches(2), Inches(2))
    before = doc.to_bytes()
    with pytest.raises(oxdocx.DocxError, match="stale or foreign"):
        doc.set_picture_size(picture, Inches(3), Inches(3))
    assert doc.to_bytes() == before


def cell_and_header_document():
    doc = oxdocx.Document()
    doc.add_paragraph("body")
    table = doc.add_table(1, 1)
    doc.section(0).header.add_paragraph("header")
    return doc, table


def test_a_cell_or_header_run_picture_takes_the_staged_route_and_reads_back():
    doc, table = cell_and_header_document()
    in_cell = table.cell(0, 0).paragraphs[0].add_run("cell ").add_picture(noise_png(8, 10), "c.png", Inches(1),
                                                                            Inches(1))
    assert in_cell.location.story.kind == "table_cell"
    doc.set_picture_size(in_cell, Inches(2), Inches(2))
    header = doc.section(0).header.paragraphs[-1]
    in_header = header.add_run("head ").add_picture(noise_png(8, 11), "h.png", Inches(1), Inches(1))
    assert in_header.location.story.kind == "header"
    fresh = [p for p in doc.pictures() if p.bytes == noise_png(8, 11)][0]
    doc.set_picture_size(fresh, Inches(3), Inches(3))
    again = reopened(doc)
    assert {p.bytes: (p.width, p.height) for p in again.pictures()} == {
        noise_png(8, 10): (Inches(2), Inches(2)), noise_png(8, 11): (Inches(3), Inches(3))}
    assert again.tables[0].cell(0, 0).text == "cell "
    assert again.paragraphs[0].text == "body"


def shared_picture_document(paragraphs, in_table=False):
    """python-docx stores the same image once: every picture shows one relationship."""
    d = docx.Document()
    image = noise_png(8, 12)
    for k in range(paragraphs):
        p = d.add_paragraph(f"shared {k} ")
        p.add_run().add_picture(io.BytesIO(image), width=docx.shared.Inches(1))
    if in_table:
        d.add_table(rows=1, cols=1).cell(0, 0).paragraphs[0].add_run().add_picture(
            io.BytesIO(image), width=docx.shared.Inches(1))
    d.add_paragraph("last")
    out = io.BytesIO()
    d.save(out)
    return oxdocx.Document.from_bytes(out.getvalue())


@pytest.mark.parametrize("paragraphs, in_table", [(1, False), (3, True), (20, False)])
def test_a_relationship_resize_reaches_every_picture_that_shows_it_and_keeps_every_handle(paragraphs, in_table):
    doc = shared_picture_document(paragraphs, in_table)
    relationship = doc.pictures()[0].relationship_id
    shown = paragraphs + in_table
    paragraph = doc.paragraphs[0]
    run = paragraph.runs[0]
    assert doc.set_picture_sizes_for_relationship(relationship, Inches(2), Inches(1)) == shown
    assert sizes(doc, relationship) == [(Inches(2), Inches(1))] * paragraphs
    run.text = "kept "
    assert doc.paragraphs[0].text == paragraph.text == "kept "
    # Repeated, and read back after a reopen.
    assert doc.set_picture_sizes_for_relationship(relationship, Inches(3), Inches(1)) == shown
    again = reopened(doc)
    assert sorted((p.width, p.height) for p in again.pictures()) == [(Inches(3), Inches(1))] * shown
    # In place, the paragraph's block is read again: it still takes its in-place route, which keeps the paragraph's
    # handle. Past 16 blocks the staged route writes the whole part, and the next block edit takes the whole route.
    run.add_picture(noise_png(8, 13), "n.png", Inches(1), Inches(1))
    assert outcome(lambda: paragraph.text) == ("kept " if paragraphs <= 16 else STALE)


def test_a_refused_relationship_resize_leaves_the_bytes_and_every_handle_unchanged(body):
    doc = oxdocx.Document.from_bytes(body)
    kept = handles(doc)
    before = doc.to_bytes()
    relationship = [p for p in doc.pictures() if p.location.story.kind == "body"][0].relationship_id
    for call in (lambda: doc.set_picture_sizes_for_relationship("rId9999", Inches(1), Inches(1)),
                 lambda: doc.set_picture_sizes_for_relationship(relationship, Inches(0), Inches(1))):
        with pytest.raises((oxdocx.DocxError, ValueError)):
            call()
        assert doc.to_bytes() == before
    assert kept[1][0].text == doc.paragraphs[EDITED].runs[0].text
    check_neighbours_kept(doc, kept)
