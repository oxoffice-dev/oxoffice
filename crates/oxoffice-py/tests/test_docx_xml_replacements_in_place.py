"""docx: `Paragraph.replace_xml`, `Run.text` and `Run.replace_xml` in a direct body paragraph, and
`SectionHandle.replace_xml`, edit their element in place.

A run edit keeps the handles of the paragraph and of every run in it, as the staged route did.

The paragraph keeps its handle and its runs retire, as `Paragraph.text` publishes it; every other paragraph, run, table,
cell, section and header handle keeps reading and editing its own element. A section replacement keeps every handle of
that section and the paragraph that holds its properties, as the staged route did; its header and footer handles retire
only when the replacement changes what the section shows. A replacement that changes nothing keeps every handle; one
that is refused leaves the bytes and every handle as they were. Reads stay equal to those of the saved document opened
again."""

import re

import pytest

import oxoffice.docx as oxdocx
from builders import edit_cost_docx

STALE = "stale"
EDITED = 3  # a body paragraph of six runs, Word's identifiers on each
HOLDER = 25  # the empty paragraph that holds the first section's properties


def outcome(read):
    try:
        return read()
    except oxdocx.StaleElementError:
        return STALE


def debug_build():
    import oxoffice.docx._native as native

    return native._debug_build


@pytest.fixture(scope="module")
def body(tmp_path_factory):
    path = edit_cost_docx(tmp_path_factory.mktemp("xml-in-place") / "body.docx", 25, probes=1)
    return path.read_bytes()


def texts(doc):
    """Every read the replacements must leave in document order, from fresh handles."""
    return {
        "paragraphs": [paragraph.text for paragraph in doc.paragraphs],
        "runs": [[run.text for run in paragraph.runs] for paragraph in doc.paragraphs],
        "cells": [[[cell.text for cell in row.cells] for row in table.rows] for table in doc.tables],
        "sections": [(section.margin_top, section.margin_left) for section in doc.sections],
        "headers": [paragraph.text for paragraph in doc.section(0).header.paragraphs],
    }


def reopened(doc):
    return oxdocx.Document.from_bytes(doc.to_bytes())


def handles(doc):
    """Handles the replacements of paragraph EDITED must keep, with the fresh read each must match."""
    fresh = {
        "before": lambda: doc.paragraphs[EDITED - 1],
        "after": lambda: doc.paragraphs[EDITED + 1],
        "after the table": lambda: doc.paragraphs[10],
        "cell": lambda: doc.tables[0].cell(1, 1).paragraphs[0],
        "header": lambda: doc.section(0).header.paragraphs[0],
    }
    taken = {label: get() for label, get in fresh.items()}
    runs = {label: paragraph.runs[0] for label, paragraph in taken.items()}
    return fresh, taken, runs


def check_kept(fresh, taken, runs):
    for label, paragraph in taken.items():
        assert paragraph.text == fresh[label]().text, label
        assert runs[label].text == fresh[label]().runs[0].text, label


# ------------------------------------------------------------------ Paragraph.replace_xml


def test_a_paragraph_xml_replacement_in_place_keeps_every_handle_outside_its_paragraph(body):
    doc = oxdocx.Document.from_bytes(body)
    paragraph = doc.paragraphs[EDITED]
    own_run = paragraph.runs[0]
    fresh, taken, runs = handles(doc)
    table, sections = doc.tables[0], [doc.section(0), doc.section(1)]

    paragraph.replace_xml(paragraph.xml.replace("of paragraph 3", "of the third paragraph"))

    assert paragraph.text == doc.paragraphs[EDITED].text
    assert paragraph.text.startswith("run 0 of the third paragraph")
    assert outcome(lambda: own_run.text) == STALE
    check_kept(fresh, taken, runs)
    assert table.cell(0, 0).text == doc.tables[0].cell(0, 0).text
    assert [section.margin_top for section in sections] == [doc.section(0).margin_top, doc.section(1).margin_top]
    # Kept handles still edit their own element, and the edited paragraph takes further edits.
    runs["after"].text = "after, edited"
    assert doc.paragraphs[EDITED + 1].runs[0].text == "after, edited"
    runs["cell"].text = "cell, edited"
    assert doc.tables[0].cell(1, 1).paragraphs[0].text.startswith("cell, edited")
    paragraph.add_run(" again")
    assert doc.paragraphs[EDITED].text.endswith(" again")
    paragraph.replace_xml(paragraph.xml.replace(" again", " once more"))
    assert doc.paragraphs[EDITED].text.endswith(" once more")
    assert texts(reopened(doc)) == texts(doc)


def test_a_paragraph_xml_replacement_that_changes_nothing_keeps_every_handle(body):
    doc = oxdocx.Document.from_bytes(body)
    paragraph = doc.paragraphs[EDITED]
    own_runs = list(paragraph.runs)
    fresh, taken, runs = handles(doc)
    before = doc.to_bytes()
    paragraph.replace_xml(paragraph.xml)
    assert doc.to_bytes() == before
    assert [run.text for run in own_runs] == [run.text for run in doc.paragraphs[EDITED].runs]
    check_kept(fresh, taken, runs)


PARAGRAPH_REFUSALS = {
    "malformed XML": lambda xml: xml[:-5],
    "another element": lambda xml: '<w:r xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"/>',
    "an unknown relationship": lambda xml: xml.replace(
        "</w:p>",
        '<w:hyperlink xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" r:id="rId999">'
        "<w:r><w:t>link</w:t></w:r></w:hyperlink></w:p>",
    ),
}


@pytest.mark.parametrize("refusal", list(PARAGRAPH_REFUSALS))
def test_a_refused_paragraph_xml_replacement_leaves_the_bytes_and_every_handle_unchanged(body, refusal):
    doc = oxdocx.Document.from_bytes(body)
    paragraph = doc.paragraphs[EDITED]
    own_runs = list(paragraph.runs)
    fresh, taken, runs = handles(doc)
    before = doc.to_bytes()
    with pytest.raises((oxdocx.DocxError, ValueError)):
        paragraph.replace_xml(PARAGRAPH_REFUSALS[refusal](paragraph.xml))
    assert doc.to_bytes() == before
    assert paragraph.text == doc.paragraphs[EDITED].text
    assert [run.text for run in own_runs] == [run.text for run in doc.paragraphs[EDITED].runs]
    check_kept(fresh, taken, runs)
    own_runs[0].text = "still mine"
    assert doc.paragraphs[EDITED].runs[0].text == "still mine"


@pytest.mark.skipif(not debug_build(), reason="a debug build fails the publication on request")
@pytest.mark.parametrize("failure", ["error", "panic"])
def test_a_paragraph_xml_replacement_whose_publication_fails_puts_the_paragraph_back(body, failure, monkeypatch):
    # The failure on request is that of the block publication: it also proves the replacement took the in-place route.
    doc = oxdocx.Document.from_bytes(body)
    paragraph = doc.paragraphs[EDITED]
    own_runs = list(paragraph.runs)
    fresh, taken, runs = handles(doc)
    before = doc.to_bytes()
    monkeypatch.setenv("OXOFFICE_DEBUG_FAIL_PARAGRAPH_SPLICE", failure)
    with pytest.raises(BaseException) as raised:
        paragraph.replace_xml(paragraph.xml.replace("of paragraph 3", "of the third paragraph"))
    assert "on request" in str(raised.value)
    monkeypatch.delenv("OXOFFICE_DEBUG_FAIL_PARAGRAPH_SPLICE")
    assert doc.to_bytes() == before
    assert [run.text for run in own_runs] == [run.text for run in doc.paragraphs[EDITED].runs]
    check_kept(fresh, taken, runs)


def test_a_paragraph_xml_replacement_that_adds_a_bookmark_keeps_its_staged_route(body):
    # A bookmark id the paragraph did not hold needs the whole-part identity check: the staged route publishes it,
    # keeping the edited paragraph's handle as before.
    doc = oxdocx.Document.from_bytes(body)
    paragraph = doc.paragraphs[EDITED]
    after = doc.paragraphs[EDITED + 1]
    paragraph.replace_xml(paragraph.xml.replace(
        "</w:p>", '<w:bookmarkStart w:id="777" w:name="added"/><w:bookmarkEnd w:id="777"/></w:p>'))
    assert paragraph.text == doc.paragraphs[EDITED].text
    assert after.text == doc.paragraphs[EDITED + 1].text
    assert 'w:name="added"' in reopened(doc).paragraphs[EDITED].xml
    assert texts(reopened(doc)) == texts(doc)


# ------------------------------------------------------------------ SectionHandle.replace_xml


def top_margin(xml, twips):
    return re.sub(r'w:top="\d+"', f'w:top="{twips}"', xml, count=1)


@pytest.mark.parametrize("index", [0, 1])
def test_a_section_xml_replacement_keeps_every_handle_outside_its_section(body, index):
    doc = oxdocx.Document.from_bytes(body)
    section = doc.section(index)
    other = doc.section(1 - index)
    same = doc.section(index)
    holder = doc.paragraphs[HOLDER]
    fresh, taken, runs = handles(doc)
    header = doc.section(0).header.paragraphs[0]

    section.replace_xml(top_margin(section.xml, 1600))

    # The handle the call was made on reads and edits the section it named.
    assert section.margin_top == doc.section(index).margin_top == 1016000
    section.margin_left = oxdocx.Inches(1.5)
    assert doc.section(index).margin_left == oxdocx.Inches(1.5)
    # The other section, every paragraph and run, the cells and the header keep their handles.
    assert other.margin_top == doc.section(1 - index).margin_top
    check_kept(fresh, taken, runs)
    assert header.text == "header"
    # Its other handles, and the paragraph that holds its properties, keep working: the same element, read again.
    assert same.margin_top == doc.section(index).margin_top
    assert holder.text == ""
    holder.add_run("tail")
    assert doc.paragraphs[HOLDER].text == "tail"
    assert texts(reopened(doc)) == texts(doc)


def test_a_section_xml_replacement_that_drops_its_header_reference_retires_its_header_handles(body):
    doc = oxdocx.Document.from_bytes(body)
    section, later = doc.section(0), doc.section(1)
    header = section.header.paragraphs[0]
    fresh, taken, runs = handles(doc)
    del taken["header"], runs["header"]
    xml = section.xml
    assert "headerReference" in xml
    section.replace_xml(re.sub(r"<w:headerReference[^>]*/>", "", xml))
    assert outcome(lambda: header.text) == STALE
    assert "headerReference" not in section.xml
    assert later.margin_top == doc.section(1).margin_top
    check_kept(fresh, taken, runs)
    copy = reopened(doc)
    assert [paragraph.text for paragraph in copy.paragraphs] == [paragraph.text for paragraph in doc.paragraphs]
    assert [s.margin_top for s in copy.sections] == [s.margin_top for s in doc.sections]


def test_a_section_xml_replacement_that_changes_nothing_keeps_every_handle(body):
    doc = oxdocx.Document.from_bytes(body)
    section, same, holder = doc.section(0), doc.section(0), doc.paragraphs[HOLDER]
    fresh, taken, runs = handles(doc)
    before = doc.to_bytes()
    section.replace_xml(section.xml)
    assert doc.to_bytes() == before
    assert section.margin_top == same.margin_top == doc.section(0).margin_top
    assert holder.text == ""
    check_kept(fresh, taken, runs)


SECTION_REFUSALS = {
    "malformed XML": lambda xml: xml[:-5],
    "another element": lambda xml: '<w:p xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"/>',
    "an unknown header relationship": lambda xml: re.sub(r'r:id="[^"]+"', 'r:id="rId999"', xml, count=1),
}


@pytest.mark.parametrize("refusal", list(SECTION_REFUSALS))
def test_a_refused_section_xml_replacement_leaves_the_bytes_and_every_handle_unchanged(body, refusal):
    doc = oxdocx.Document.from_bytes(body)
    section, same, holder = doc.section(0), doc.section(0), doc.paragraphs[HOLDER]
    fresh, taken, runs = handles(doc)
    before = doc.to_bytes()
    with pytest.raises((oxdocx.DocxError, ValueError)):
        section.replace_xml(SECTION_REFUSALS[refusal](section.xml))
    assert doc.to_bytes() == before
    assert section.margin_top == same.margin_top == doc.section(0).margin_top
    assert holder.text == ""
    check_kept(fresh, taken, runs)
    same.margin_left = oxdocx.Inches(1.25)
    assert doc.section(0).margin_left == oxdocx.Inches(1.25)


def test_a_section_xml_replacement_on_a_new_document_keeps_the_other_handles():
    doc = oxdocx.Document()
    doc.add_paragraph("first")
    doc.insert_section(len(doc.sections))
    doc.add_paragraph("last")
    first = doc.paragraphs[0]
    run = first.runs[0]
    section = doc.section(1)
    section.replace_xml(top_margin(section.xml, 2000))
    assert doc.section(1).margin_top == section.margin_top
    assert run.text == "first" and first.text == "first"
    assert texts(reopened(doc))["sections"] == texts(doc)["sections"]


# ------------------------------------------------------------------ Run.text and Run.replace_xml

RUN = 2  # the run edited in paragraph EDITED

RUN_EDITS = {
    "Run.text": lambda run: setattr(run, "text", "a new run text "),
    "Run.replace_xml": lambda run: run.replace_xml(run.xml.replace("run 2 of", "RUN 2 of")),
}


def run_handles(doc):
    paragraph = doc.paragraphs[EDITED]
    return paragraph, list(paragraph.runs)


def check_runs_kept(doc, paragraph, own_runs):
    assert paragraph.text == doc.paragraphs[EDITED].text
    assert [run.text for run in own_runs] == [run.text for run in doc.paragraphs[EDITED].runs]


@pytest.mark.parametrize("call", list(RUN_EDITS))
def test_a_run_edit_in_place_keeps_the_paragraph_and_every_run_handle(body, call):
    doc = oxdocx.Document.from_bytes(body)
    paragraph, own_runs = run_handles(doc)
    fresh, taken, runs = handles(doc)

    RUN_EDITS[call](own_runs[RUN])

    expected = "a new run text " if call == "Run.text" else "RUN 2 of paragraph 3 "
    assert own_runs[RUN].text == expected
    check_runs_kept(doc, paragraph, own_runs)
    check_kept(fresh, taken, runs)
    # The edited run, its siblings and the paragraph take further edits.
    own_runs[RUN].text = "again "
    own_runs[RUN + 1].text = "sibling "
    assert [run.text for run in doc.paragraphs[EDITED].runs][RUN : RUN + 2] == ["again ", "sibling "]
    own_runs[0].replace_xml(own_runs[0].xml.replace("run 0 of", "RUN 0 of"))
    assert doc.paragraphs[EDITED].runs[0].text.startswith("RUN 0 of")
    paragraph.add_run("tail")
    assert doc.paragraphs[EDITED].text.endswith("tail")
    runs["after"].text = "after, edited"
    assert doc.paragraphs[EDITED + 1].runs[0].text == "after, edited"
    assert texts(reopened(doc)) == texts(doc)


@pytest.mark.parametrize("call", ["Run.text", "Run.replace_xml"])
def test_a_run_edit_that_changes_nothing_keeps_every_handle(body, call):
    doc = oxdocx.Document.from_bytes(body)
    # A run Word saved carries identifiers that setting its text drops: the first setting is an edit.
    if call == "Run.text":
        doc.paragraphs[EDITED].runs[RUN].text = "the same text "
    paragraph, own_runs = run_handles(doc)
    fresh, taken, runs = handles(doc)
    before = doc.to_bytes()
    if call == "Run.text":
        own_runs[RUN].text = own_runs[RUN].text
    else:
        own_runs[RUN].replace_xml(own_runs[RUN].xml)
    assert doc.to_bytes() == before
    check_runs_kept(doc, paragraph, own_runs)
    check_kept(fresh, taken, runs)


RUN_REFUSALS = {
    "Run.text with a character XML cannot carry": lambda run: setattr(run, "text", "a\x01b"),
    "Run.replace_xml with malformed XML": lambda run: run.replace_xml(run.xml[:-5]),
    "Run.replace_xml with another element": lambda run: run.replace_xml(
        '<w:p xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"/>'
    ),
    "Run.replace_xml with two runs": lambda run: run.replace_xml(run.xml + run.xml),
}


@pytest.mark.parametrize("refusal", list(RUN_REFUSALS))
def test_a_refused_run_edit_leaves_the_bytes_and_every_handle_unchanged(body, refusal):
    doc = oxdocx.Document.from_bytes(body)
    paragraph, own_runs = run_handles(doc)
    fresh, taken, runs = handles(doc)
    before = doc.to_bytes()
    with pytest.raises((oxdocx.DocxError, ValueError)):
        RUN_REFUSALS[refusal](own_runs[RUN])
    assert doc.to_bytes() == before
    check_runs_kept(doc, paragraph, own_runs)
    check_kept(fresh, taken, runs)
    own_runs[RUN].text = "still mine "
    assert doc.paragraphs[EDITED].runs[RUN].text == "still mine "


@pytest.mark.skipif(not debug_build(), reason="a debug build fails the publication on request")
@pytest.mark.parametrize("failure", ["error", "panic"])
@pytest.mark.parametrize("call", list(RUN_EDITS))
def test_a_run_edit_whose_publication_fails_puts_the_paragraph_back(body, call, failure, monkeypatch):
    # The failure on request is that of the block publication: it also proves the edit took the in-place route.
    doc = oxdocx.Document.from_bytes(body)
    paragraph, own_runs = run_handles(doc)
    fresh, taken, runs = handles(doc)
    before = doc.to_bytes()
    monkeypatch.setenv("OXOFFICE_DEBUG_FAIL_PARAGRAPH_SPLICE", failure)
    with pytest.raises(BaseException) as raised:
        RUN_EDITS[call](own_runs[RUN])
    assert "on request" in str(raised.value)
    monkeypatch.delenv("OXOFFICE_DEBUG_FAIL_PARAGRAPH_SPLICE")
    assert doc.to_bytes() == before
    check_runs_kept(doc, paragraph, own_runs)
    check_kept(fresh, taken, runs)


@pytest.mark.parametrize("call", list(RUN_EDITS))
@pytest.mark.parametrize("where", ["table cell", "content control"])
def test_a_run_edit_outside_a_direct_body_paragraph_keeps_its_route(body, call, where):
    doc = oxdocx.Document.from_bytes(body)
    if where == "table cell":
        target = lambda: doc.tables[0].cell(1, 1).paragraphs[0]  # noqa: E731
    else:
        index = next(index for index, p in enumerate(doc.paragraphs) if p.text == "control 0")
        target = lambda: doc.paragraphs[index]  # noqa: E731
    run = target().runs[0]
    after = doc.paragraphs[EDITED + 1].runs[0]
    edit = {
        "Run.text": lambda run: setattr(run, "text", "edited elsewhere"),
        "Run.replace_xml": lambda run: run.replace_xml(run.xml.replace("</w:t>", " edited</w:t>", 1)),
    }[call]
    edit(run)
    assert "edited" in target().runs[0].text
    assert after.text == doc.paragraphs[EDITED + 1].runs[0].text
    assert texts(reopened(doc)) == texts(doc)
