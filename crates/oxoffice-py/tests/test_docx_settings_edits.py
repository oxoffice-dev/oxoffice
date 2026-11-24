"""docx: an edit of the settings part (font flags, document protection, the document-wide note policy, distinct
even-page headers) or of a section's properties (page borders, the section's note policy) changes no element a handle
names, so it keeps every handle: a handle taken before the edit reads and edits the same element after it. The edit is
made in place, without saving the document: a refused edit leaves the document as it was, byte for byte."""
import io
import zipfile

import pytest

import oxoffice.docx as oxdocx
from builders import edit_cost_docx
from test_docx_block_handles import outcome

EDITS = {
    "embed_true_type_fonts": lambda doc: setattr(doc, "embed_true_type_fonts", True),
    "embed_system_fonts": lambda doc: setattr(doc, "embed_system_fonts", False),
    "save_subset_fonts": lambda doc: setattr(doc, "save_subset_fonts", True),
    "set_document_protection": lambda doc: doc.set_document_protection(
        oxdocx.DocumentProtection(mode="readOnly", enforcement=True)),
    "remove_document_protection": lambda doc: (
        doc.set_document_protection(oxdocx.DocumentProtection(mode="readOnly", enforcement=True)),
        handles_taken(doc),
        doc.remove_document_protection(),
    ),
    "set_note_policy": lambda doc: doc.set_note_policy("footnote", format="lowerRoman", start=3),
    "odd_and_even_pages_header_footer": lambda doc: setattr(doc.settings, "odd_and_even_pages_header_footer", True),
    "section set_page_border": lambda doc: doc.section(0).set_page_border("left", style="double"),
    "section set_note_policy": lambda doc: doc.section(0).set_note_policy("endnote", format="upperLetter"),
}


@pytest.fixture(scope="module")
def saved(tmp_path_factory):
    """A body whose paragraphs, runs and rows carry Word's identifiers, two sections, the first with a header."""
    return edit_cost_docx(tmp_path_factory.mktemp("settings") / "saved.docx", 12)


TAKEN = {}


def handles_taken(doc):
    """A handle of each kind the binding tracks, with what it reads now."""
    table = doc.tables[0]
    taken = {
        "paragraph": (doc.paragraphs[3], lambda h: h.text),
        "run": (doc.paragraphs[3].runs[2], lambda h: h.text),
        "table": (table, lambda h: h.cell(1, 1).text),
        "cell": (table.cell(0, 1), lambda h: h.text),
        "cell paragraph": (table.cell(1, 0).paragraphs[0], lambda h: h.text),
        "header paragraph": (doc.section(0).header.paragraphs[0], lambda h: h.text),
        "section": (doc.section(0), lambda h: h.page_width),
    }
    TAKEN[id(doc)] = {name: (handle, read, read(handle)) for name, (handle, read) in taken.items()}


@pytest.mark.parametrize("name", sorted(EDITS))
def test_a_settings_edit_keeps_every_handle(saved, name):
    doc = oxdocx.Document.open(saved)
    handles_taken(doc)
    EDITS[name](doc)
    taken = TAKEN.pop(id(doc))
    assert {key: outcome(lambda: read(handle)) for key, (handle, read, _) in taken.items()} == {
        key: before for key, (_, _, before) in taken.items()}
    # The handles still edit their element.
    taken["paragraph"][0].text = "edited paragraph"
    taken["cell"][0].text = "edited cell"
    assert doc.paragraphs[3].text == "edited paragraph"
    assert doc.tables[0].cell(0, 1).text == "edited cell"


def test_a_settings_edit_is_read_back_after_a_save(saved):
    doc = oxdocx.Document.open(saved)
    for edit in EDITS.values():
        if edit is not EDITS["remove_document_protection"]:
            edit(doc)
    again = oxdocx.Document.from_bytes(doc.to_bytes())
    assert 'w:val="double"' in again.section(0).xml
    assert (again.embed_true_type_fonts, again.embed_system_fonts, again.save_subset_fonts) == (True, False, True)
    assert again.settings.odd_and_even_pages_header_footer
    assert again.document_protection.mode == "readOnly"


def with_settings(source, settings):
    """`source` with its settings part replaced by `settings`."""
    out = io.BytesIO()
    with zipfile.ZipFile(source) as zin, zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as zout:
        for item in zin.infolist():
            data = zin.read(item.filename)
            zout.writestr(item, settings if item.filename == "word/settings.xml" else data)
    return out.getvalue()


W = 'xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"'
TWICE = '<w:documentProtection w:edit="readOnly" w:enforcement="1"/>' * 2


def test_a_refused_settings_edit_leaves_the_document_as_it_was(saved):
    """Two protection elements: removing one would reinterpret the other, so the removal is refused."""
    doc = oxdocx.Document.from_bytes(with_settings(saved, f"<w:settings {W}>{TWICE}</w:settings>".encode()))
    paragraph = doc.paragraphs[3]
    before = doc.to_bytes()
    with pytest.raises(Exception, match="ambiguous or malformed"):
        doc.remove_document_protection()
    assert doc.to_bytes() == before
    assert paragraph.text == doc.paragraphs[3].text
