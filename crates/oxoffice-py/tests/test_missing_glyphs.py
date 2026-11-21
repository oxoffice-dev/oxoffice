"""Text that no font in use can draw is drawn as empty boxes, and the diagnostics of a conversion say which script and
how to get it drawn: system fonts or a font file of the caller's. The same message as the Rust facade and the CLI."""

from pathlib import Path

import oxoffice.docx as oxdocx
import oxoffice.pptx as oxpptx

CORPUS = Path(__file__).resolve().parents[3] / "tests" / "corpus" / "files"
WAYS_OUT = ("--system-fonts", "system_fonts=True", "RenderOptions::with_system_fonts", "font file")


def boxes(messages):
    return [message for message in messages if "drawn as empty boxes" in message]


def test_a_document_names_the_script_it_draws_as_boxes():
    doc = oxdocx.Document()
    doc.add_paragraph("我们的公司在北京")
    doc.add_paragraph("Latin stays drawn.")
    _, messages = doc.to_pdf_with_diagnostics()
    (message,) = boxes(messages)
    assert message.startswith("Chinese, Japanese or Korean ideograph (Han) text has no glyph"), message
    assert all(way in message for way in WAYS_OUT), message


def test_a_document_the_bundled_fonts_draw_says_nothing():
    doc = oxdocx.Document()
    doc.add_paragraph("Plain Latin, مرحبا, שלום")
    _, messages = doc.to_pdf_with_diagnostics()
    assert not boxes(messages), messages


def test_a_deck_names_the_scripts_it_draws_as_boxes():
    _, messages = oxpptx.Presentation(CORPUS / "powerpoint" / "multilingual.pptx").to_pdf_with_diagnostics()
    found = boxes(messages)
    assert any(message.startswith("Chinese, Japanese or Korean ideograph (Han)") for message in found), messages
    assert all(all(way in message for way in WAYS_OUT) for message in found), found
