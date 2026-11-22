"""A value holding a character XML 1.0 cannot carry is refused at the call with XmlError, which is also a ValueError
(as python-docx and python-pptx raise), and the document is left unchanged and still saves."""

import pytest

import oxoffice.docx as oxdocx
import oxoffice.pptx as oxpptx
from oxoffice.pptx.util import Inches

BAD = "a\x07b"


def test_xml_error_is_also_a_value_error():
    assert issubclass(oxdocx.XmlError, oxdocx.DocxError)
    assert issubclass(oxdocx.XmlError, ValueError)
    assert issubclass(oxpptx.XmlError, oxpptx.PptxError)
    assert issubclass(oxpptx.XmlError, ValueError)


def word_document():
    doc = oxdocx.Document()
    paragraph = doc.add_paragraph("first")
    paragraph.add_run("run")
    table = doc.add_table(rows=1, cols=1)
    return doc, paragraph, table


WORD_EDITS = {
    "Document.add_paragraph": lambda doc, paragraph, table: doc.add_paragraph(BAD),
    "Paragraph.add_run": lambda doc, paragraph, table: paragraph.add_run(BAD),
    "Paragraph.add_hyperlink text": lambda doc, paragraph, table: paragraph.add_hyperlink(BAD, "https://example.org/"),
    "Paragraph.add_hyperlink url": lambda doc, paragraph, table: paragraph.add_hyperlink("link", "https://ex\x07.org/"),
    "Paragraph.text": lambda doc, paragraph, table: setattr(paragraph, "text", BAD),
    "Run.text": lambda doc, paragraph, table: setattr(paragraph.runs[0], "text", BAD),
    "Font.name": lambda doc, paragraph, table: setattr(paragraph.runs[0].font, "name", BAD),
    "Cell.text": lambda doc, paragraph, table: setattr(table.cell(0, 0), "text", BAD),
    "Cell.add_paragraph": lambda doc, paragraph, table: table.cell(0, 0).add_paragraph(BAD),
    "header.add_paragraph": lambda doc, paragraph, table: doc.section(0).header.add_paragraph(BAD),
    "Document.add_style": lambda doc, paragraph, table: doc.add_style("Bad", font_name=BAD),
    "core_properties.title": lambda doc, paragraph, table: setattr(doc.core_properties, "title", BAD),
}


@pytest.mark.parametrize("name", WORD_EDITS)
def test_a_word_setter_refuses_a_character_xml_cannot_carry(name):
    doc, paragraph, table = word_document()
    before = doc.to_bytes()
    with pytest.raises(oxdocx.XmlError, match="U\\+0007"):
        WORD_EDITS[name](doc, paragraph, table)
    assert doc.to_bytes() == before


def powerpoint_paragraph():
    prs = oxpptx.Presentation()
    slide = prs.slides.add_slide(prs.slide_layouts[6])
    box = slide.shapes.add_textbox(0, 0, Inches(2), Inches(1))
    box.text_frame.text = "text"
    return prs, box.text_frame.paragraphs[0]


POWERPOINT_EDITS = {
    "core_properties.title": lambda prs, paragraph: setattr(prs.core_properties, "title", BAD),
    "Font.name": lambda prs, paragraph: setattr(paragraph.runs[0].font, "name", BAD),
    "Paragraph.bullet": lambda prs, paragraph: setattr(paragraph, "bullet", "\x07"),
}


@pytest.mark.parametrize("name", POWERPOINT_EDITS)
def test_a_powerpoint_value_xml_cannot_carry_is_refused_as_an_xml_error(name):
    prs, paragraph = powerpoint_paragraph()
    before = prs.to_bytes()
    with pytest.raises(oxpptx.XmlError, match="U\\+0007"):
        POWERPOINT_EDITS[name](prs, paragraph)
    assert prs.to_bytes() == before
