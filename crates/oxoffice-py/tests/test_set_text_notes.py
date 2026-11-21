"""Setting the text of a paragraph, a cell or a run keeps its footnote marks, so no note is left without one."""

import io
import re
import zipfile

import oxoffice.docx as oxdocx


def saved(doc):
    data = doc.to_bytes()
    return oxdocx.Document.from_bytes(data), zipfile.ZipFile(io.BytesIO(data)).read("word/document.xml")


def footnote_texts(doc):
    return [p.text.strip() for p in doc.story_paragraphs if p.kind == "footnote"]


def test_a_paragraph_keeps_its_note_mark_when_its_text_is_set():
    doc = oxdocx.Document()
    doc.add_footnote(doc.add_paragraph("old"), "note")
    doc.paragraphs[0].text = "new"
    doc, xml = saved(doc)
    assert doc.paragraphs[0].text == "new"
    assert footnote_texts(doc) == ["note"]
    assert b"footnoteReference" in xml


def test_a_cell_keeps_its_note_mark_when_its_text_is_set():
    doc = oxdocx.Document()
    table = doc.add_table(1, 1)
    doc.add_footnote(table.cell(0, 0).paragraphs[0], "cell note")
    doc.tables[0].cell(0, 0).text = "new"
    doc, xml = saved(doc)
    assert doc.tables[0].cell(0, 0).text == "new"
    assert footnote_texts(doc) == ["cell note"]
    assert b"footnoteReference" in xml


def test_the_find_and_replace_loop_over_runs_keeps_the_note_mark():
    doc = oxdocx.Document()
    doc.add_footnote(doc.add_paragraph("old"), "note")
    for run in doc.paragraphs[0].runs:
        run.text = run.text.replace("old", "new")
    doc, xml = saved(doc)
    assert doc.paragraphs[0].text == "new"
    assert footnote_texts(doc) == ["note"]
    assert b"footnoteReference" in xml


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
