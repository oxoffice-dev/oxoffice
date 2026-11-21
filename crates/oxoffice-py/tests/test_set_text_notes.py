"""Setting the text of a paragraph, a cell or a run keeps its footnote marks, so no note is left without one."""

import io
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
