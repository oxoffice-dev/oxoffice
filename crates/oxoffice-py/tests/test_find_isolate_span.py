"""Paragraph.find_text finds a match in a paragraph's literal run text, and Document.isolate_span splits the runs so
that it covers whole runs that can be formatted, on the Word and Google Docs files of the corpus."""

from pathlib import Path

import pytest

import oxoffice.docx as oxdocx

CORPUS = Path(__file__).resolve().parents[3] / "tests" / "corpus" / "files"
PRODUCERS = ["word", "google"]


def report(producer):
    return oxdocx.Document.from_bytes((CORPUS / producer / "report.docx").read_bytes())


def summary_index(doc):
    return next(i for i, p in enumerate(doc.paragraphs) if p.text.startswith("Summary. Revenue grew"))


def bold_characters(paragraph):
    """Whether each character of the paragraph's runs is bold by direct formatting."""
    return [run.bold is True for run in paragraph.runs for _ in run.text]


@pytest.mark.parametrize("producer", PRODUCERS)
def test_find_text_gives_the_span_of_the_chosen_match(producer):
    doc = report(producer)
    paragraph = doc.paragraphs[summary_index(doc)]
    span = paragraph.find_text("the")
    assert paragraph.text[span.start:span.end] == "the"
    assert span.paragraph.text == paragraph.text
    second = paragraph.find_text("the", 1)
    assert second.start >= span.end
    assert paragraph.find_text("not in the paragraph") is None
    assert paragraph.find_text("") is None
    assert paragraph.find_text("the", 99) is None


@pytest.mark.parametrize("producer", PRODUCERS)
def test_isolate_span_makes_whole_runs_that_can_be_formatted(producer):
    doc = report(producer)
    index = summary_index(doc)
    before = doc.paragraphs[index].text
    was_bold = bold_characters(doc.paragraphs[index])
    span = doc.paragraphs[index].find_text("grew by 12 %")
    first, last = doc.isolate_span(span)
    paragraph = doc.paragraphs[index]
    assert "".join(run.text for run in paragraph.runs[first:last]) == "grew by 12 %"
    assert paragraph.text == before
    for run in paragraph.runs[first:last]:
        run.bold = True
    saved = oxdocx.Document.from_bytes(doc.to_bytes()).paragraphs[index]
    assert saved.text == before
    expected = [bold or span.start <= at < span.end for at, bold in enumerate(was_bold)]
    assert bold_characters(saved) == expected


def owner_paragraph(doc, owner):
    return doc.tables[0].cell(1, 0).paragraphs[0] if owner == "cell" else doc.section(0).header.paragraphs[0]


@pytest.mark.parametrize("producer", PRODUCERS)
@pytest.mark.parametrize(("owner", "word"), [("cell", "revenue"), ("header", "Analytics")])
def test_a_span_in_a_table_cell_or_a_header_is_isolated_and_saved(producer, owner, word):
    doc = report(producer)
    paragraph = owner_paragraph(doc, owner)
    text = paragraph.text
    assert text != word and len(paragraph.runs) == 1, "the span splits a run"
    span = paragraph.find_text(word)
    first, last = doc.isolate_span(span)
    # The change stales earlier handles: read the paragraph again.
    paragraph = owner_paragraph(doc, owner)
    assert "".join(run.text for run in paragraph.runs[first:last]) == word
    for run in paragraph.runs[first:last]:
        run.bold = True
    saved = owner_paragraph(oxdocx.Document.from_bytes(doc.to_bytes()), owner)
    assert saved.text == text
    assert bold_characters(saved) == [span.start <= at < span.end for at in range(len(text))]


def test_a_span_of_another_document_is_refused():
    doc, other = report("word"), report("word")
    span = other.paragraphs[summary_index(other)].find_text("grew")
    with pytest.raises(ValueError, match="different document"):
        doc.isolate_span(span)


# Isolating a span of a direct body paragraph edits that paragraph in place: its handle stays and its runs retire,
# as for the other paragraph edits; every other handle keeps reading and editing its own element.
EDITED = 3  # a body paragraph of six runs, Word's identifiers on each


def outcome(read):
    try:
        return read()
    except oxdocx.StaleElementError:
        return "stale"


@pytest.fixture(scope="module")
def body(tmp_path_factory):
    from builders import edit_cost_docx

    return edit_cost_docx(tmp_path_factory.mktemp("span") / "body.docx", 25, probes=1).read_bytes()


def body_texts(doc):
    return [[run.text for run in paragraph.runs] for paragraph in doc.paragraphs]


def test_isolating_a_span_of_a_body_paragraph_keeps_the_handles_of_other_paragraphs(body):
    doc = oxdocx.Document.from_bytes(body)
    paragraph = doc.paragraphs[EDITED]
    text = paragraph.text
    own_runs = list(paragraph.runs)
    others = {index: doc.paragraphs[index].runs[0] for index in (EDITED - 1, EDITED + 1, 10)}
    cell = doc.tables[0].cell(1, 1).paragraphs[0].runs[0]
    word = text.split()[1]
    span = paragraph.find_text(word)

    first, last = doc.isolate_span(span)

    assert "".join(run.text for run in paragraph.runs[first:last]) == word
    assert paragraph.text == text
    assert [outcome(lambda: run.text) for run in own_runs] == ["stale"] * len(own_runs)
    for index, run in others.items():
        assert run.text == doc.paragraphs[index].runs[0].text
    assert cell.text == doc.tables[0].cell(1, 1).paragraphs[0].runs[0].text
    # The kept handles still edit their own element, and a later span of the same paragraph isolates too.
    others[EDITED + 1].text = "after, edited"
    assert doc.paragraphs[EDITED + 1].runs[0].text == "after, edited"
    first, last = doc.isolate_span(paragraph.find_text(word))
    paragraph.runs[first].bold = True
    assert body_texts(oxdocx.Document.from_bytes(doc.to_bytes())) == body_texts(doc)


def test_isolating_a_span_that_already_covers_whole_runs_keeps_the_bytes_and_every_handle(body):
    doc = oxdocx.Document.from_bytes(body)
    paragraph = doc.paragraphs[EDITED]
    runs = list(paragraph.runs)
    whole = runs[1].text
    before = doc.to_bytes()
    first, last = doc.isolate_span(paragraph.find_text(whole))
    assert (first, last) == (1, 2)
    assert doc.to_bytes() == before
    assert [run.text for run in runs] == [run.text for run in doc.paragraphs[EDITED].runs]


def test_a_span_past_the_text_of_its_body_paragraph_is_refused_and_keeps_every_handle(body):
    doc = oxdocx.Document.from_bytes(body)
    paragraph = doc.paragraphs[EDITED]
    span = paragraph.find_text(paragraph.text[-4:])
    # Setting the text edits in place too: the paragraph handle and the span stay bound, the span now past the text.
    paragraph.text = "short"
    runs, other = list(paragraph.runs), doc.paragraphs[EDITED + 1].runs[0]
    before = doc.to_bytes()
    with pytest.raises(oxdocx.DocxError):
        doc.isolate_span(span)
    assert doc.to_bytes() == before
    assert [run.text for run in runs] == ["short"]
    assert other.text == doc.paragraphs[EDITED + 1].runs[0].text
