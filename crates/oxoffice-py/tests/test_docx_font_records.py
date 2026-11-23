"""A font name written twice, as LibreOffice writes Symbol, is changed and removed by its index in `fonts`."""

from pathlib import Path

import pytest

import oxoffice.docx as oxdocx

CORPUS = Path(__file__).resolve().parents[3] / "tests" / "corpus" / "files"
FACE = Path(__file__).resolve().parents[2] / "oxoffice-layout" / "fonts" / "NotoSansSymbols2-bullets-subset.ttf"
LICENSE = oxdocx.FontEmbeddingLicense(authorized=True, identity="SIL Open Font License 1.1")


def test_a_font_name_written_twice_is_changed_and_removed_by_its_index():
    doc = oxdocx.Document.from_bytes((CORPUS / "libreoffice" / "report.docx").read_bytes())
    names = [font.name for font in doc.fonts]
    assert [i for i, name in enumerate(names) if name == "Symbol"] == [1, 8]

    for call in (lambda: doc.set_font(oxdocx.FontDefinition("Symbol")), lambda: doc.remove_font("Symbol"),
                 lambda: doc.embed_font("Symbol", FACE.read_bytes(), license=LICENSE),
                 lambda: doc.remove_embedded_font("Symbol")):
        with pytest.raises(oxdocx.DocxError, match="2 records are named Symbol"):
            call()
    with pytest.raises(oxdocx.DocxError, match="does not rename"):
        doc.set_font_at(8, oxdocx.FontDefinition("Wingdings"))
    with pytest.raises(oxdocx.DocxError, match="out of range"):
        doc.remove_font_at(9)

    doc.set_font_at(8, oxdocx.FontDefinition("Symbol", family="decorative"))
    doc.embed_font_at(8, FACE.read_bytes(), license=LICENSE)
    doc = oxdocx.Document.from_bytes(doc.to_bytes())
    assert (doc.fonts[1].family, len(doc.fonts[1].embedded_fonts)) == ("roman", 0)
    assert (doc.fonts[8].family, len(doc.fonts[8].embedded_fonts)) == ("decorative", 1)

    assert doc.remove_embedded_font_at(8).kind == "regular"
    assert doc.remove_embedded_font_at(8) is None
    assert doc.remove_font_at(8).family == "decorative"
    assert [font.name for font in doc.fonts] == names[:8]
