"""Text the bundled fonts cannot draw: a document takes a system font for it by default and names it; with
bundled_fonts_only=True, or when no system font has it, it is drawn as empty boxes and the diagnostics say which script
and how to get it drawn. A deck takes a font file. The same messages as the Rust facade and the CLI."""

import pytest


from pathlib import Path

import oxoffice.docx as oxdocx
import oxoffice.pptx as oxpptx

CORPUS = Path(__file__).resolve().parents[3] / "tests" / "corpus" / "files"
WAYS_OUT = ("--bundled-fonts-only", "bundled_fonts_only=False", "with_bundled_fonts_only(false)", "font file")
DECK_WAYS_OUT = ("--font-dir", "fonts=", "font_dir=", "font file")


def boxes(messages):
    return [message for message in messages if "drawn as empty boxes" in message]


def test_a_bundled_only_document_names_the_script_it_draws_as_boxes():
    doc = oxdocx.Document()
    doc.add_paragraph("我们的公司在北京")
    doc.add_paragraph("Latin stays drawn.")
    _, messages = doc.to_pdf_with_diagnostics(bundled_fonts_only=True)
    (message,) = boxes(messages)
    assert message.startswith("Chinese, Japanese or Korean ideograph (Han) text has no glyph"), message
    assert all(way in message for way in WAYS_OUT), message


def test_a_document_takes_a_system_font_or_says_none_was_found():
    # Depends on the machine's fonts, so either branch passes, each with its own line.
    doc = oxdocx.Document()
    doc.add_paragraph("我们的公司在北京")
    _, messages = doc.to_pdf_with_diagnostics()
    han = [message for message in messages if message.startswith("Chinese, Japanese or Korean ideograph (Han)")]
    assert han, messages
    for message in han:
        assert "is drawn with the system font '" in message or (
            "drawn as empty boxes" in message and "no system font on this machine has them" in message
        ), message


def test_a_document_the_bundled_fonts_draw_says_nothing_and_is_the_same_in_both_modes():
    doc = oxdocx.Document()
    doc.add_paragraph("Plain Latin, مرحبا, שלום")
    pdf, messages = doc.to_pdf_with_diagnostics()
    assert not boxes(messages), messages
    assert not any("system font '" in message for message in messages), messages
    assert pdf == doc.to_pdf(bundled_fonts_only=True)


def test_system_fonts_and_bundled_fonts_only_exclude_each_other():
    with pytest.raises(ValueError, match="exclude each other"):
        oxdocx.Document().to_pdf(system_fonts=True, bundled_fonts_only=True)


def test_a_deck_names_the_scripts_it_draws_as_boxes():
    _, messages = oxpptx.Presentation(CORPUS / "powerpoint" / "multilingual.pptx").to_pdf_with_diagnostics()
    found = boxes(messages)
    assert any(message.startswith("Chinese, Japanese or Korean ideograph (Han)") for message in found), messages
    assert all(all(way in message for way in DECK_WAYS_OUT) for message in found), found
    assert not any("system_fonts" in message for message in found), found


def test_a_deck_notes_export_names_the_boxes_once():
    deck = oxpptx.Presentation(CORPUS / "powerpoint" / "multilingual.pptx")
    _, messages = deck.to_notes_pdf_with_diagnostics()
    han = [message for message in boxes(messages) if message.startswith("Chinese, Japanese or Korean ideograph")]
    assert len(han) == 1, messages
