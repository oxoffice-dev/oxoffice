"""A note policy set without a number format takes Word's default of its family."""

import io
import re
import zipfile

import oxoffice.docx as oxdocx


def note_formats(doc):
    """The w:numFmt values of the saved settings and section properties."""
    archive = zipfile.ZipFile(io.BytesIO(doc.to_bytes()))
    xml = archive.read("word/settings.xml") + archive.read("word/document.xml")
    return {
        family: re.findall(rb"<w:" + family + rb'Pr>.*?<w:numFmt w:val="(\w+)"', xml)
        for family in (b"footnote", b"endnote")
    }


def test_a_note_policy_without_a_format_numbers_as_word_does():
    # Word numbers footnotes 1, 2, 3 and endnotes i, ii, iii: moving the
    # notes must not renumber them.
    doc = oxdocx.Document()
    doc.add_paragraph("Body")
    doc.set_note_policy("endnote", placement="sectEnd")
    doc.set_note_policy("footnote", placement="beneathText")
    assert note_formats(doc) == {b"footnote": [b"decimal"], b"endnote": [b"lowerRoman"]}
    section = oxdocx.Document()
    section.add_paragraph("Body")
    section.section(0).set_note_policy("endnote", placement="docEnd")
    assert note_formats(section)[b"endnote"] == [b"lowerRoman"]
    doc.set_note_policy("endnote", format="decimal")
    assert note_formats(doc)[b"endnote"] == [b"decimal"]
