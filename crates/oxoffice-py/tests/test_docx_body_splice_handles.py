"""docx: inserting or removing body blocks keeps the handles of every other block. A handle to a paragraph or a table
after the edited slot follows its block to its new place: it reads and edits that block, never the one that now sits
where it was. A handle to a removed block raises the stale-handle error, and so does a run handle, as after an
append. The inserted block's handle works at once.

Removals keep handles where the body splice takes them, on bodies whose paragraphs carry no identifiers; a body Word
saved still takes the whole route, which retires every handle (oxoffice-dev/oxoffice#466)."""
import pytest

import oxoffice.docx as oxdocx
from test_docx_block_handles import IDS, STALE, control, document, outcome, p, table


def body(ids):
    """Paragraphs p0 to p4 with a 2x2 table t0 after p1 and a 1x1 table t1 after p3."""
    return document(
        p("p0", " tail")
        + p("p1", " tail")
        + table2("t0")
        + p("p2", " tail")
        + p("p3", " tail")
        + table("<w:p><w:r><w:t>t1 a</w:t></w:r></w:p>")
        + p("p4", " tail"),
        ids=ids,
    )


def table2(name):
    cell = lambda text: f"<w:tc><w:p><w:r><w:t>{name} {text}</w:t></w:r></w:p></w:tc>"  # noqa: E731
    grid = '<w:tblGrid><w:gridCol w:w="2000"/><w:gridCol w:w="2000"/></w:tblGrid>'
    return f"<w:tbl>{grid}<w:tr>{cell('a')}{cell('b')}</w:tr><w:tr>{cell('c')}{cell('d')}</w:tr></w:tbl>"


def runs_text(paragraph):
    """A paragraph's text read through its runs, where the handle's place is used: `Paragraph.text` may come from a
    cache keyed by the paragraph's owner, which reads the right text even from a handle that lost its place."""
    return "".join(run.text for run in paragraph.runs)


def handles(doc):
    """A handle of every kind on blocks before and after the slots the tests edit, with what each one reads."""
    t0, t1 = doc.tables[0], doc.tables[1]
    taken = {
        "p0": (doc.paragraphs[0], runs_text),
        "p2": (doc.paragraphs[2], runs_text),
        "p4": (doc.paragraphs[4], runs_text),
        "p4 runs": (doc.paragraphs[4].runs, lambda h: [run.text for run in h]),
        "p4 format": (doc.paragraphs[4].paragraph_format, lambda h: h.alignment),
        "t0": (t0, lambda h: h.cell(1, 1).text),
        "t0 rows": (t0.rows, lambda h: len(h)),
        "t0 row": (t0.rows[1], lambda h: [cell.text for cell in h.cells]),
        "t0 cells": (t0.rows[1].cells, lambda h: h[0].text),
        "t0 cell": (t0.cell(0, 1), lambda h: h.text),
        "t0 cell paragraphs": (t0.cell(0, 1).paragraphs, lambda h: h[0].text),
        "t0 cell paragraph": (t0.cell(0, 1).paragraphs[0], runs_text),
        "t1": (t1, lambda h: h.cell(0, 0).text),
        "t1 cell paragraph": (t1.cell(0, 0).paragraphs[0], runs_text),
    }
    return {name: (handle, read, read(handle)) for name, (handle, read) in taken.items()}


def check(taken, retired=()):
    """Every handle reads what it read before, but those of `retired`, which raise the stale-handle error."""
    for name, (handle, read, before) in taken.items():
        assert outcome(lambda: read(handle)) == (STALE if name in retired else before), name


def texts(doc):
    return [paragraph.text for paragraph in doc.paragraphs]


@IDS
@pytest.mark.parametrize("slot", [0, 1, 3])
def test_a_paragraph_inserted_keeps_the_handles_of_every_block(slot, ids):
    doc = body(ids)
    taken = handles(doc)
    run = doc.paragraphs[4].runs[0]
    inserted = doc.insert_paragraph(slot, "new")
    check(taken)
    assert outcome(lambda: run.text) == STALE
    assert inserted.text == "new"
    # An edit through a moved handle reaches its own block.
    taken["p4"][0].text = "p4 edited"
    taken["t0 cell paragraph"][0].text = "t0 b edited"
    assert texts(doc)[-1] == "p4 edited"
    assert doc.tables[0].cell(0, 1).text == "t0 b edited"
    assert texts(doc).count("new") == 1


@IDS
def test_a_paragraph_inserted_before_another_keeps_the_handles_of_every_block(ids):
    doc = body(ids)
    taken = handles(doc)
    inserted = taken["p2"][0].insert_paragraph_before("new")
    check(taken)
    assert inserted.text == "new"
    assert texts(doc)[2:4] == ["new", "p2 tail"]


@IDS
@pytest.mark.parametrize("slot", [0, 2, 5])
def test_a_table_inserted_keeps_the_handles_of_every_block(slot, ids):
    doc = body(ids)
    taken = handles(doc)
    inserted = doc.insert_table(slot, 1, 1)
    check(taken)
    inserted.cell(0, 0).text = "new cell"
    taken["t1"][0].cell(0, 0).text = "t1 edited"
    assert [table.cell(0, 0).text for table in doc.tables].count("new cell") == 1
    assert doc.tables[-1].cell(0, 0).text == "t1 edited"


def test_a_paragraph_removed_retires_its_handles_only():
    doc = body(ids=False)
    taken = handles(doc)
    assert doc.remove_content(3)  # p2, after p0, p1 and t0
    check(taken, retired={"p2"})
    taken["p4"][0].text = "p4 edited"
    assert texts(doc) == ["p0 tail", "p1 tail", "p3 tail", "p4 edited"]


def test_a_range_of_paragraphs_removed_retires_their_handles_only():
    doc = body(ids=False)
    taken = handles(doc)
    assert doc.remove_content_range(3, 5) == 2  # p2 and p3
    check(taken, retired={"p2"})
    assert texts(doc) == ["p0 tail", "p1 tail", "p4 tail"]
    assert taken["t1"][0].cell(0, 0).text == "t1 a"


def test_the_last_paragraph_removed_retires_its_handles_only():
    doc = body(ids=False)
    taken = handles(doc)
    assert doc.remove_content(len(doc.paragraphs) + len(doc.tables) - 1)  # p4
    check(taken, retired={"p4", "p4 runs", "p4 format"})
    assert texts(doc) == ["p0 tail", "p1 tail", "p2 tail", "p3 tail"]


@IDS
def test_handles_follow_their_block_through_several_edits(ids):
    doc = body(ids)
    taken = handles(doc)
    doc.insert_paragraph(0, "first")
    doc.insert_table(1, 1, 1)
    doc.insert_paragraph(4, "middle")
    doc.insert_table(9, 1, 1)
    if not ids:
        assert doc.remove_content(0)
    check(taken)
    saved = oxdocx.Document.from_bytes(doc.to_bytes())
    assert texts(saved) == texts(doc)


@IDS
def test_a_table_after_a_table_in_a_block_content_control_follows_its_own_index(ids):
    # The table in the control has no owner, so the later table's owner counts one table fewer than doc.tables does.
    doc = document(table(p("a")) + control(table(p("b"))) + p("x") + table(p("d")), ids=ids)
    later = doc.tables[-1]
    paragraph = later.cell(0, 0).paragraphs[0]
    doc.insert_paragraph(0, "new")
    assert later.cell(0, 0).text == "d"
    assert runs_text(paragraph) == "d"
    paragraph.text = "d edited"
    assert doc.tables[-1].cell(0, 0).text == "d edited"
    assert doc.tables[1].cell(0, 0).text == "b"


@IDS
def test_a_paragraph_in_nested_block_content_controls_reads_its_own_text_or_raises_after_an_insertion(ids):
    centered = '<w:pPr><w:jc w:val="center"/></w:pPr>'
    doc = document(p("before") + control(p("outer", " two"), control(p(centered, "inner", " two"))) + p("after"),
                   ids=ids)
    taken = {paragraph.text: paragraph for paragraph in doc.paragraphs}
    alignment = {text: paragraph.paragraph_format.alignment for text, paragraph in taken.items()}
    assert alignment["inner two"] == oxdocx.WD_ALIGN_PARAGRAPH.CENTER
    doc.insert_paragraph(0, "new")
    for text, paragraph in taken.items():
        assert outcome(lambda: runs_text(paragraph)) in (text, STALE), text
        assert outcome(lambda: paragraph.paragraph_format.alignment) in (alignment[text], STALE), text
    assert runs_text(taken["after"]) == "after"


def test_removing_the_paragraph_that_ends_a_section_retires_the_section_handles():
    # Removing it merges two sections: a kept section handle would read and edit the section that took its place.
    doc = oxdocx.Document()
    doc.add_paragraph("a")
    doc.insert_section(len(doc.sections))
    assert len(doc.sections) == 2
    first = doc.section(0)
    doc.section(1).margin_top = oxdocx.Inches(2)
    assert doc.remove_content(len(doc.paragraphs) + len(doc.tables) - 1)
    assert len(doc.sections) == 1
    assert outcome(lambda: first.margin_top) == STALE


def test_removing_the_last_paragraph_after_a_run_was_added_to_an_earlier_one_keeps_the_other_handles():
    # The run the XML replacement adds is born after the owners of later paragraphs.
    doc = oxdocx.Document()
    first = doc.add_paragraph("")
    second = doc.add_paragraph("b")
    doc.add_paragraph("last")
    run = "<w:r><w:t>x</w:t></w:r>"
    xml = first.xml
    first.replace_xml(xml[:-2] + f">{run}</w:p>" if xml.endswith("/>") else xml.replace("</w:p>", f"{run}</w:p>"))
    assert runs_text(first) == "x"
    assert doc.remove_content(2)
    assert runs_text(first) == "x"
    assert runs_text(second) == "b"
    assert texts(doc) == ["x", "b"]
