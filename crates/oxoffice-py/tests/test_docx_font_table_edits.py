"""docx: an edit of the font table (a record set, removed or embedded with a face, a face removed) changes no element a
handle names, so it keeps every handle. It is made in place, without saving the document: it costs the font table,
not the body, and a refused edit leaves the document as it was, byte for byte."""
import io
import re
import zipfile

import pytest
from PIL import ImageFont

import oxoffice.docx as oxdocx
from builders import edit_cost_docx
from test_docx_block_handles import outcome

FONT = ImageFont.load_default(12).font_bytes
LICENSE = oxdocx.FontEmbeddingLicense(authorized=True, identity="CC0-1.0: Aileron")


def embedded(doc, name):
    doc.embed_font(name, FONT, license=LICENSE)


EDITS = {
    "set_font": lambda doc: doc.set_font(oxdocx.FontDefinition("Probe", family="roman")),
    "set_font_at": lambda doc: (doc.set_font(oxdocx.FontDefinition("Probe")),
                                doc.set_font_at(index_of(doc, "Probe"), oxdocx.FontDefinition("Probe", pitch="fixed"))),
    "remove_font": lambda doc: (doc.set_font(oxdocx.FontDefinition("Gone")), doc.remove_font("Gone")),
    "remove_font_at": lambda doc: (doc.set_font(oxdocx.FontDefinition("Gone")), doc.remove_font_at(index_of(doc, "Gone"))),
    "embed_font": lambda doc: embedded(doc, "Aileron"),
    "embed_font_at": lambda doc: (doc.set_font(oxdocx.FontDefinition("Bare")),
                                  doc.embed_font_at(index_of(doc, "Bare"), FONT, license=LICENSE)),
    # A second face, then the first one again, which changes nothing.
    "embed_font again": lambda doc: (embedded(doc, "Aileron"), doc.embed_font("Aileron", FONT, "bold", license=LICENSE),
                                     embedded(doc, "Aileron")),
    "remove_embedded_font": lambda doc: (embedded(doc, "Faced"), doc.remove_embedded_font("Faced")),
    "remove_embedded_font_at": lambda doc: (embedded(doc, "Faced"), doc.remove_embedded_font_at(index_of(doc, "Faced"))),
}


def index_of(doc, name):
    return next(i for i, font in enumerate(doc.fonts) if font.name == name)


@pytest.fixture(scope="module")
def saved(tmp_path_factory):
    return edit_cost_docx(tmp_path_factory.mktemp("fonts") / "saved.docx", 12)


def handles_taken(doc):
    table = doc.tables[0]
    taken = {
        "paragraph": (doc.paragraphs[3], lambda h: h.text),
        "run": (doc.paragraphs[3].runs[2], lambda h: h.text),
        "table": (table, lambda h: h.cell(1, 1).text),
        "cell": (table.cell(0, 1), lambda h: h.text),
        "header paragraph": (doc.section(0).header.paragraphs[0], lambda h: h.text),
        "section": (doc.section(0), lambda h: h.page_width),
    }
    return {name: (handle, read, read(handle)) for name, (handle, read) in taken.items()}


@pytest.mark.parametrize("name", sorted(EDITS))
def test_a_font_table_edit_keeps_every_handle(saved, name):
    doc = oxdocx.Document.open(saved)
    taken = handles_taken(doc)
    EDITS[name](doc)
    assert {key: outcome(lambda: read(handle)) for key, (handle, read, _) in taken.items()} == {
        key: before for key, (_, _, before) in taken.items()}
    taken["paragraph"][0].text = "edited paragraph"
    assert doc.paragraphs[3].text == "edited paragraph"


def test_font_table_edits_read_back_after_a_save(saved):
    doc = oxdocx.Document.open(saved)
    for name in ["set_font", "remove_font", "embed_font", "embed_font again", "remove_embedded_font"]:
        EDITS[name](doc)
    again = oxdocx.Document.from_bytes(doc.to_bytes())
    fonts = {font.name: font for font in again.fonts}
    assert fonts["Probe"].family == "roman"
    assert "Gone" not in fonts
    assert sorted(face.kind for face in fonts["Aileron"].embedded_fonts) == ["bold", "regular"]
    assert fonts["Faced"].embedded_fonts == ()
    assert again.embed_true_type_fonts
    parts = zipfile.ZipFile(io.BytesIO(again.to_bytes())).namelist()
    # The removed face's part goes with it; the two faces kept have one part each.
    assert len([part for part in parts if re.match(r"word/fonts/font\d+\.odttf", part)]) == 2


W = 'xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"'


def with_part(source, name, data):
    out = io.BytesIO()
    with zipfile.ZipFile(source) as zin, zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as zout:
        for item in zin.infolist():
            zout.writestr(item, data if item.filename == name else zin.read(item.filename))
    return out.getvalue()


FACE = ('<w:embedRegular xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" '
        'r:id="rIdMissing" w:fontKey="{00000000-0000-0000-0000-000000000000}"/>')
REFUSALS = {
    # A font table the model cannot own (two regular faces in one record) is kept verbatim: the edit is refused when
    # it reserves the font table.
    "unmodeled font table": ("word/fontTable.xml", f'<w:fonts {W}><w:font w:name="Twice">{FACE}{FACE}</w:font></w:fonts>'),
    # Two w:embedTrueTypeFonts: the face's part name and relationship are reserved first, then the settings refuse.
    "ambiguous font setting": ("word/settings.xml", f"<w:settings {W}><w:embedTrueTypeFonts/><w:embedTrueTypeFonts/></w:settings>"),
}


@pytest.mark.parametrize("error", sorted(REFUSALS))
def test_a_refused_font_table_edit_leaves_the_document_as_it_was(saved, error):
    part, xml = REFUSALS[error]
    doc = oxdocx.Document.from_bytes(with_part(saved, part, xml.encode()))
    paragraph = doc.paragraphs[3]
    before = doc.to_bytes()
    with pytest.raises(Exception, match=error):
        doc.embed_font("Twice", FONT, license=LICENSE)
    assert doc.to_bytes() == before
    assert paragraph.text == doc.paragraphs[3].text
    # The reservations were undone: the next edit takes the names the refused one had reserved.
    if error == "ambiguous font setting":
        doc.set_font(oxdocx.FontDefinition("After"))
        assert "After" in [font.name for font in doc.fonts]
