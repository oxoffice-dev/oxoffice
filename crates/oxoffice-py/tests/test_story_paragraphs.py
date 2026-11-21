"""Document.story_paragraphs lists every paragraph of every story with its story kind, part and style, in the
accepted view, and Document.count_text counts a text by story kind, on the Word and Google Docs files of the
corpus."""

import io
import zipfile
from pathlib import Path

import pytest

import oxoffice.docx as oxdocx

CORPUS = Path(__file__).resolve().parents[3] / "tests" / "corpus" / "files"
PRODUCERS = ["word", "google"]

# A table of contents in a block content control, and a match of ACME across the edge of an inline control.
EXTRA = (
    '<w:sdt><w:sdtPr><w:docPartObj><w:docPartGallery w:val="Table of Contents"/></w:docPartObj></w:sdtPr>'
    '<w:sdtContent><w:p><w:pPr><w:pStyle w:val="TOC1"/></w:pPr><w:r><w:t>ACME entry</w:t></w:r></w:p>'
    '<w:p><w:pPr><w:pStyle w:val="TOC2"/></w:pPr><w:r><w:t>Second entry</w:t></w:r></w:p></w:sdtContent></w:sdt>'
    '<w:p><w:r><w:t>AC</w:t></w:r><w:sdt><w:sdtContent><w:r><w:t>ME split</w:t></w:r></w:sdtContent></w:sdt></w:p>'
    '<w:p><w:r><w:t xml:space="preserve">kept </w:t></w:r>'
    '<w:del w:id="902" w:author="a" w:date="2026-01-01T00:00:00Z"><w:r><w:delText>gone</w:delText></w:r></w:del></w:p>'
)


def report(producer, extra=""):
    data = (CORPUS / producer / "report.docx").read_bytes()
    if not extra:
        return oxdocx.Document.from_bytes(data)
    out = io.BytesIO()
    with zipfile.ZipFile(io.BytesIO(data)) as source, zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as target:
        for item in source.infolist():
            part = source.read(item.filename)
            if item.filename == "word/document.xml":
                xml = part.decode("utf-8")
                at = xml.rfind("<w:sectPr")
                part = (xml[:at] + extra + xml[at:]).encode("utf-8")
            target.writestr(item, part)
    return oxdocx.Document.from_bytes(out.getvalue())


def find(paragraphs, text):
    found = [p for p in paragraphs if p.text == text]
    assert found, f"no paragraph {text!r}"
    return found[0]


@pytest.mark.parametrize("producer", PRODUCERS)
def test_every_story_is_listed_with_its_kind_part_and_style(producer):
    paragraphs = report(producer).story_paragraphs
    heading = find(paragraphs, "1. Highlights")
    assert (heading.kind, heading.part_name) == ("body", "/word/document.xml")
    assert heading.style == ("Titre1" if producer == "word" else "Heading1")
    cell = find(paragraphs, "Indicator")
    assert (cell.kind, cell.story.kind) == ("body", "table_cell")
    header = find(paragraphs, "Northwind Analytics: Quarterly Operations Review")
    assert (header.kind, header.part_name) == ("header", "/word/header1.xml")
    assert find(paragraphs, "Internal. Prepared by the operations team.").kind == "footer"
    comment = find(paragraphs, "Confirm with finance before sending.")
    assert (comment.kind, comment.style) == ("comment", None)
    assert repr(heading).startswith('StoryParagraph(kind="body", part_name="/word/document.xml"')


@pytest.mark.parametrize("producer", PRODUCERS)
def test_block_controls_are_read_paragraph_by_paragraph_in_the_accepted_view(producer):
    paragraphs = report(producer, EXTRA).story_paragraphs
    entry = find(paragraphs, "ACME entry")
    assert entry.style == "TOC1" and len(entry.index_path) == 2
    assert find(paragraphs, "Second entry").style == "TOC2"
    find(paragraphs, "ACME split")
    find(paragraphs, "kept ")
    assert not any("gone" in p.text for p in paragraphs)


@pytest.mark.parametrize("producer", PRODUCERS)
def test_counts_by_story_kind_include_what_replacement_cannot_reach(producer):
    doc = report(producer, EXTRA)
    doc.add_footnote(doc.paragraphs[0], "ACME in a note")
    assert doc.count_text("ACME") == {"body": 2, "footnote": 1}
    assert list(doc.count_text("the")) == ["body", "footer"]
    assert doc.count_text("not in the report") == {}
    # The match across the edge of the inline control is counted, not replaced.
    assert doc.replace_all([("ACME", "Globex", 2)]) == (2,)
    assert doc.count_text("ACME") == {"body": 1}


def test_counting_an_empty_text_is_refused():
    with pytest.raises(ValueError, match="count_text"):
        report("word").count_text("")
