"""Bibliography from Python: sources built, added and read back by value, the style, citations at a run boundary or
after a piece of text, a bibliography, and the update of their results, with the same refusals as the CLI."""

import pytest

import oxoffice.docx as oxdocx


def smith(**kwargs):
    return oxdocx.BibliographySource(
        "Smith20",
        "book",
        contributors=[oxdocx.BibliographyContributor("author", people=[oxdocx.BibliographyPerson("Smith", "Ada")])],
        fields=[("title", "A history"), ("year", "2020"), ("publisher", "Press")],
        **kwargs,
    )


def document(*paragraphs):
    doc = oxdocx.Document()
    for text in paragraphs:
        doc.add_paragraph(text)
    return doc


def reopened(doc):
    return oxdocx.Document.from_bytes(doc.to_bytes())


def test_a_source_reads_back_as_the_value_that_was_added():
    doe = oxdocx.BibliographySource(
        "Doe19",
        "journal_article",
        guid="{11111111-2222-4333-8444-555555555555}",
        locale=1033,
        contributors=[
            oxdocx.BibliographyContributor("author", people=[oxdocx.BibliographyPerson(["Doe"], ["Jane", "Q"])]),
            oxdocx.BibliographyContributor("author", corporate="WHO"),
        ],
        fields=[("title", "A study"), ("year", "2019"), ("journal_name", "Liver"), ("pages", "1-9")],
    )
    doc = document("Text")
    guid = doc.add_bibliography_source(smith())
    assert guid.startswith("{") and guid.endswith("}") and len(guid) == 38
    assert doc.add_bibliography_source(doe) == "{11111111-2222-4333-8444-555555555555}"

    first, second = reopened(doc).bibliography_sources()
    assert (first.tag, first.guid, first.source_type, first.diagnostics) == ("Smith20", guid, "Book", ())
    assert first.supported == smith(guid=guid)
    assert second.supported == doe
    person = second.supported.contributors[0].people[0]
    assert (person.last, person.first, person.middle) == (("Doe",), ("Jane", "Q"), ())
    assert second.supported.contributors[1].corporate == "WHO"
    assert second.supported.contributors[1].people is None
    assert second.supported.fields[2] == ("journal_name", "Liver")

    # A source built again without its GUID replaces the one with its tag and keeps the GUID.
    edited = oxdocx.BibliographySource("Smith20", "book", fields=[("title", "A longer history")])
    doc.replace_bibliography_source(edited)
    first = doc.bibliography_sources()[0]
    assert first.guid == guid
    assert first.supported.fields == (("title", "A longer history"),)
    doc.remove_bibliography_source("Doe19")
    assert [source.tag for source in reopened(doc).bibliography_sources()] == ["Smith20"]


def test_a_cited_document_gets_its_citation_and_bibliography_results():
    doc = document("One brave step", "Two brave words")
    doc.add_bibliography_source(smith())
    doc.set_bibliography_options("apa6")
    doc.insert_citation_after_text("brave", ["Smith20"], occurrence=1, locale=1033)
    doc.insert_bibliography(2, "apa6")
    with pytest.raises(oxdocx.DocxError, match="requires application default locale"):
        doc.update_bibliography()

    report = doc.update_bibliography(default_locale=1033)
    assert (report.updated_citations, report.rebuilt_bibliographies) == (1, 1)
    doc = reopened(doc)
    texts = [paragraph.text for paragraph in doc.paragraphs]
    assert texts[:2] == ["One brave step", "Two brave (Smith, 2020) words"]
    assert any("Smith, A. (2020)" in text for text in texts[2:]), texts
    style = doc.bibliography_style()
    assert (style.supported_style, style.style_key) == ("apa6", "APA")
    with pytest.raises(oxdocx.DocxError, match="source is referenced by a citation"):
        doc.remove_bibliography_source("Smith20")


def test_a_citation_at_a_run_boundary_carries_each_source_switches():
    doc = document("Cited here")
    doc.add_bibliography_source(smith())
    doc.set_bibliography_options("apa6")
    position = oxdocx.StoryRunPosition(paragraph=doc.paragraphs[0], run_index=1)
    doc.insert_citation(
        position,
        [oxdocx.CitationSource("Smith20", pages="12, 14", suppress_author=True)],
        locale=1033,
    )
    report = doc.update_bibliography()
    assert report.updated_citations == 1
    assert doc.paragraphs[0].text == "Cited here (A history, 2020, p. 12, 14)"


def test_names_and_argument_shapes_are_refused_before_anything_changes():
    with pytest.raises(ValueError, match="unknown source kind 'Book'|unknown source kind \"Book\""):
        oxdocx.BibliographySource("Smith20", "Book")
    with pytest.raises(ValueError, match="unknown source field"):
        oxdocx.BibliographySource("Smith20", "book", fields=[("RefOrder", "1")])
    with pytest.raises(ValueError, match="a field is a"):
        oxdocx.BibliographySource("Smith20", "book", fields=[("title",)])
    with pytest.raises(ValueError, match="unknown contributor role"):
        oxdocx.BibliographyContributor("Author", corporate="WHO")
    with pytest.raises(ValueError, match="exactly one of people and corporate"):
        oxdocx.BibliographyContributor("author")
    with pytest.raises(TypeError, match="sequence"):
        oxdocx.BibliographyContributor("author", people=oxdocx.BibliographyPerson("Smith"))

    doc = document("Text")
    doc.add_bibliography_source(smith())
    before = doc.to_bytes()
    with pytest.raises(ValueError, match="apa6, chicago, gb7714"):
        doc.set_bibliography_options("harvard-anglia")
    with pytest.raises(TypeError, match="tags must be a sequence"):
        doc.set_bibliography_options("apa6", tags="Smith20")
    with pytest.raises(TypeError, match="sources must be a sequence"):
        doc.insert_citation_after_text("Text", "Smith20")
    with pytest.raises(oxdocx.DocxError, match="has no occurrence 1: it occurs 1 time"):
        doc.insert_citation_after_text("Text", ["Smith20"], occurrence=1)
    assert doc.to_bytes() == before
