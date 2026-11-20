"""save() refuses to replace the file a document or presentation was opened from, after any edit, unless
overwrite_source=True, and saves anywhere else as before."""

import io
import os
import shutil
from pathlib import Path

import pytest

import oxoffice.docx as oxdocx
import oxoffice.pptx as oxpptx

CORPUS = Path(__file__).resolve().parents[3] / "tests" / "corpus" / "files"


@pytest.fixture
def report(tmp_path):
    path = tmp_path / "report.docx"
    shutil.copy(CORPUS / "word" / "report.docx", path)
    return path


@pytest.fixture
def deck(tmp_path):
    path = tmp_path / "deck.pptx"
    shutil.copy(CORPUS / "powerpoint" / "deck.pptx", path)
    return path


def assert_refused(target, path):
    before = Path(path).read_bytes()
    with pytest.raises(oxdocx.PackageError if isinstance(target, oxdocx.Document) else oxpptx.PackageError,
                       match="overwrite_source"):
        target.save(path)
    assert Path(path).read_bytes() == before


def test_a_document_refuses_its_source_after_edits(report):
    doc = oxdocx.Document(report)
    assert_refused(doc, report)
    assert_refused(doc, str(report))
    doc.add_paragraph("added")
    assert_refused(doc, report)
    doc.paragraphs[-1].runs[0].text = "changed"
    assert_refused(doc, report)
    doc.add_table(2, 2)
    assert_refused(doc, report)
    doc.core_properties.title = "A title"
    assert_refused(doc, report)
    doc.to_bytes()
    assert_refused(doc, report)


def test_a_document_refuses_its_source_through_a_link(report, tmp_path):
    if not hasattr(os, "symlink") or os.name == "nt":
        pytest.skip("symbolic links need privileges on Windows")
    link = tmp_path / "link.docx"
    link.symlink_to(report)
    assert_refused(oxdocx.Document(report), link)
    assert_refused(oxdocx.Document(link), report)


def test_a_document_overwrites_its_source_when_asked(report):
    doc = oxdocx.Document(report)
    doc.add_paragraph("written over the source")
    doc.save(report, overwrite_source=True)
    assert "written over the source" in oxdocx.Document(report).paragraphs[-1].text


def test_a_document_saves_elsewhere_and_to_a_stream(report, tmp_path):
    before = report.read_bytes()
    doc = oxdocx.Document(report)
    doc.add_paragraph("only in the copy")
    doc.save(tmp_path / "copy.docx")
    stream = io.BytesIO()
    doc.save(stream)
    assert report.read_bytes() == before
    assert oxdocx.Document(tmp_path / "copy.docx").paragraphs[-1].text == "only in the copy"
    assert oxdocx.Document(io.BytesIO(stream.getvalue())).paragraphs[-1].text == "only in the copy"


def test_a_document_read_from_a_stream_has_no_source(report):
    with open(report, "rb") as f:
        doc = oxdocx.Document(f)
    doc.save(report)


def test_overwrite_source_is_keyword_only(report):
    with pytest.raises(TypeError):
        oxdocx.Document(report).save(report, True)


def test_a_presentation_refuses_its_source_after_edits(deck):
    prs = oxpptx.Presentation(deck)
    assert_refused(prs, deck)
    slide = prs.slides.add_slide(prs.slide_layouts[len(prs.slide_layouts) - 1])
    assert_refused(prs, deck)
    box = slide.shapes.add_textbox(oxpptx.Inches(1), oxpptx.Inches(1), oxpptx.Inches(2), oxpptx.Inches(1))
    box.text_frame.text = "added"
    assert_refused(prs, deck)
    prs.core_properties.title = "A title"
    assert_refused(prs, deck)
    prs.to_bytes()
    assert_refused(prs, deck)


def test_a_presentation_overwrites_its_source_when_asked(deck):
    prs = oxpptx.Presentation(deck)
    count = len(prs.slides)
    prs.slides.add_slide(prs.slide_layouts[0])
    prs.save(deck, overwrite_source=True)
    assert len(oxpptx.Presentation(deck).slides) == count + 1


def test_a_presentation_from_bytes_has_no_source(deck):
    prs = oxpptx.Presentation.from_bytes(deck.read_bytes())
    prs.save(deck)
