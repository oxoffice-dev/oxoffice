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


def instructions(doc):
    """The field instructions of the body, each run of spaces read as one."""
    xml = doc.part_xml("/word/document.xml")
    texts = [part.split(">", 1)[1].split("</w:instrText>", 1)[0] for part in xml.split("<w:instrText")[1:]]
    return [" ".join(text.replace("&quot;", '"').split()) for text in texts]


def test_selectors_and_several_cited_sources_reach_the_fields():
    doc = document("First", "Second")
    doc.add_bibliography_source(smith())
    doc.add_bibliography_source(oxdocx.BibliographySource("Doe19", "book", fields=[["title", "Notes"], ["year", "2019"]]))
    body = next(story for story in doc.stories if story.kind == "body")
    doc.insert_bibliography(body, "apa6", locale=1033, locale_filter=1036, tags=["Doe19"])
    doc.insert_citation_after_text("First", [
        oxdocx.CitationSource("Smith20", volume="2", prefix="see ", suppress_year=True),
        oxdocx.CitationSource("Doe19", suffix=" more", suppress_title=True),
    ])
    assert instructions(doc) == [
        r'CITATION Smith20 \v 2 \f "see " \y \m Doe19 \s " more" \t',
        r"BIBLIOGRAPHY \l 1033 \f 1036 \m Doe19",
    ]
    doc.set_bibliography_options("chicago", tags=["Smith20"])
    assert instructions(doc)[1] == r"BIBLIOGRAPHY \m Smith20"
    assert doc.bibliography_style().supported_style == "chicago"


def test_a_listed_source_round_trips_and_a_replacement_names_its_missing_source():
    doc = document("Text")
    assert doc.bibliography_style() is None
    with pytest.raises(oxdocx.DocxError, match='source "Smith20": no source has this tag'):
        doc.replace_bibliography_source(smith())
    # Pairs as `json.load` reads `sources --json`: lists.
    listed = {"tag": "Smith20", "kind": "book", "fields": [["title", "A history"], ["year", "2020"]]}
    doc.add_bibliography_source(oxdocx.BibliographySource(listed["tag"], listed["kind"], fields=listed["fields"]))
    assert doc.bibliography_sources()[0].supported.fields == (("title", "A history"), ("year", "2020"))


def test_adding_and_replacing_a_source_keeps_every_handle_bound():
    doc = document("First", "Second")
    first, second = doc.paragraphs
    run = second.runs[0]
    guid = doc.add_bibliography_source(smith())
    doc.add_bibliography_source(oxdocx.BibliographySource("Doe19", "book", fields=[("title", "Notes")]))
    doc.replace_bibliography_source(oxdocx.BibliographySource("Smith20", "book", fields=[("title", "Another")]))
    first.text = "First edited"
    run.text = "Second edited"
    back = reopened(doc)
    assert [paragraph.text for paragraph in back.paragraphs] == ["First edited", "Second edited"]
    smith_back = back.bibliography_sources()[0]
    assert (smith_back.tag, smith_back.guid) == ("Smith20", guid)
    assert smith_back.supported.fields == (("title", "Another"),)


def test_a_replacement_that_changes_nothing_writes_nothing():
    doc = document("Text")
    doc.add_bibliography_source(smith())
    paragraph = doc.paragraphs[0]
    before = doc.to_bytes()
    doc.replace_bibliography_source(smith())
    assert doc.to_bytes() == before
    paragraph.text = "Still bound"
    assert reopened(doc).paragraphs[0].text == "Still bound"


def test_a_refused_source_leaves_the_document_unchanged():
    doc = document("Text")
    paragraph = doc.paragraphs[0]
    doc.add_bibliography_source(smith())
    before = doc.to_bytes()
    with pytest.raises(oxdocx.DocxError, match="source identity already exists"):
        doc.add_bibliography_source(smith(guid="{11111111-2222-4333-8444-555555555555}"))
    with pytest.raises(oxdocx.DocxError, match="source GUID cannot change"):
        doc.replace_bibliography_source(smith(guid="{11111111-2222-4333-8444-555555555555}"))
    assert doc.to_bytes() == before
    assert [source.tag for source in doc.bibliography_sources()] == ["Smith20"]
    paragraph.text = "Still bound"
    assert reopened(doc).paragraphs[0].text == "Still bound"


# A citation in a direct body paragraph edits that paragraph in place: its handle stays and its runs retire, as for
# the other paragraph edits; every other handle keeps reading and editing its own element. Elsewhere (a table cell, a
# header, a position the document did not list) it takes the staged route.
EDITED = 3  # a body paragraph of six runs, Word's identifiers on each


def outcome(read):
    try:
        return read()
    except oxdocx.StaleElementError:
        return "stale"


@pytest.fixture(scope="module")
def body(tmp_path_factory):
    from builders import edit_cost_docx

    return edit_cost_docx(tmp_path_factory.mktemp("citation") / "body.docx", 25, probes=1).read_bytes()


def cited_body(body):
    doc = oxdocx.Document.from_bytes(body)
    doc.add_bibliography_source(smith())
    doc.set_bibliography_options("apa6")
    return doc


def run_texts(doc):
    return [[run.text for run in paragraph.runs] for paragraph in doc.paragraphs]


def test_a_citation_in_a_body_paragraph_keeps_the_handles_of_other_paragraphs(body):
    doc = cited_body(body)
    paragraph = doc.paragraphs[EDITED]
    own_runs = list(paragraph.runs)
    text, first = paragraph.text, own_runs[0].text
    others = {index: doc.paragraphs[index].runs[0] for index in (EDITED - 1, EDITED + 1, 10)}
    cell = doc.tables[0].cell(1, 1).paragraphs[0].runs[0]
    position = oxdocx.StoryRunPosition(paragraph=paragraph, run_index=1)

    doc.insert_citation(position, ["Smith20"], locale=1033)

    assert [outcome(lambda: run.text) for run in own_runs] == ["stale"] * len(own_runs)
    for index, run in others.items():
        assert run.text == doc.paragraphs[index].runs[0].text
    assert cell.text == doc.tables[0].cell(1, 1).paragraphs[0].runs[0].text
    others[EDITED + 1].text = "after, edited"
    assert doc.paragraphs[EDITED + 1].runs[0].text == "after, edited"
    # The edited paragraph's handle stays and takes further edits.
    paragraph.add_run(" again")
    assert doc.update_bibliography().updated_citations == 1
    assert doc.paragraphs[EDITED].text == first + " (Smith, 2020)" + text[len(first):] + " again"
    assert run_texts(reopened(doc)) == run_texts(doc)


@pytest.mark.parametrize("refusal", ["past the runs", "no source"])
def test_a_refused_citation_in_a_body_paragraph_leaves_the_bytes_and_every_handle(body, refusal):
    doc = cited_body(body)
    paragraph = doc.paragraphs[EDITED]
    runs, other = list(paragraph.runs), doc.paragraphs[EDITED + 1].runs[0]
    before = doc.to_bytes()
    run_index, sources = (99, ["Smith20"]) if refusal == "past the runs" else (1, [])
    with pytest.raises(oxdocx.DocxError):
        doc.insert_citation(oxdocx.StoryRunPosition(paragraph=paragraph, run_index=run_index), sources)
    assert doc.to_bytes() == before
    assert [run.text for run in runs] == [run.text for run in doc.paragraphs[EDITED].runs]
    assert other.text == doc.paragraphs[EDITED + 1].runs[0].text
    runs[0].text = "still mine"
    assert doc.paragraphs[EDITED].runs[0].text == "still mine"


def test_a_stale_position_in_a_body_paragraph_is_refused_and_changes_nothing(body):
    doc = cited_body(body)
    position = oxdocx.StoryRunPosition(paragraph=doc.paragraphs[EDITED], run_index=1)
    # A paragraph inserted before it moves every later body item: the listed position is stale.
    doc.insert_paragraph(0, "inserted first")
    before = doc.to_bytes()
    with pytest.raises(oxdocx.StaleElementError):
        doc.insert_citation(position, ["Smith20"], locale=1033)
    assert doc.to_bytes() == before


def body_item(doc, predicate):
    return next(item for item in doc.story_items if predicate(item))


@pytest.mark.parametrize("owner", ["cell", "header", "caller-built"])
def test_a_citation_outside_a_listed_body_paragraph_takes_the_staged_route(body, owner):
    doc = cited_body(body)
    if owner == "cell":
        item = body_item(doc, lambda it: it.story.kind == "table_cell" and it.kind == "paragraph")
    elif owner == "header":
        item = body_item(doc, lambda it: it.story.kind == "header" and it.kind == "paragraph")
    else:
        listed = oxdocx.StoryRunPosition(paragraph=doc.paragraphs[EDITED], run_index=1).item
        item = oxdocx.StoryItem(story=listed.story, kind=listed.kind, index_path=listed.index_path,
                                text=listed.text, direct_body_index=listed.direct_body_index,
                                revision=listed.revision)
    text = item.text
    doc.insert_citation(oxdocx.StoryRunPosition(item=item, run_index=0), ["Smith20"], locale=1033)
    assert doc.update_bibliography().updated_citations == 1
    cited = [it.text.strip() for it in reopened(doc).story_items if it.text and it.text.endswith(text)]
    assert f"(Smith, 2020){text}".strip() in cited


TRACKED = 'w:author="Ada" w:date="2026-11-24T09:00:00Z"'
SHAPES = {
    "tracked insertion": f'<w:r><w:t>A</w:t></w:r><w:ins w:id="5" {TRACKED}><w:r><w:t>B</w:t></w:r></w:ins>'
                         '<w:r><w:t>C</w:t></w:r>',
    "tracked deletion": f'<w:r><w:t>A</w:t></w:r><w:del w:id="6" {TRACKED}><w:r><w:delText>B</w:delText></w:r></w:del>'
                        '<w:r><w:t>C</w:t></w:r>',
    "complex field": '<w:r><w:t>A</w:t></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r>'
                     '<w:r><w:instrText xml:space="preserve"> PAGE </w:instrText></w:r>'
                     '<w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>1</w:t></w:r>'
                     '<w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:t>C</w:t></w:r>',
    "simple field": '<w:r><w:t>A</w:t></w:r><w:fldSimple w:instr=" PAGE "><w:r><w:t>1</w:t></w:r></w:fldSimple>'
                    '<w:r><w:t>C</w:t></w:r>',
}


def with_body(body):
    """A new document whose body is `body`."""
    import io
    import zipfile

    source = oxdocx.Document().to_bytes()
    out = io.BytesIO()
    with zipfile.ZipFile(io.BytesIO(source)) as package, zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as target:
        for item in package.infolist():
            part = package.read(item.filename)
            if item.filename == "word/document.xml":
                xml = part.decode("utf-8")
                start = xml.index("<w:body>") + len("<w:body>")
                end = xml.find("<w:sectPr")
                part = (xml[:start] + body + xml[end if end >= 0 else xml.index("</w:body>"):]).encode("utf-8")
            target.writestr(item, part)
    return oxdocx.Document.from_bytes(out.getvalue())


# Debug builds compare every in-place citation with the staged route's (same saved parts, or the same refusal), so
# each boundary of these shapes proves the typed insertion writes what the byte splice writes.
@pytest.mark.parametrize("shape", list(SHAPES))
def test_a_citation_next_to_tracked_changes_and_fields_matches_the_staged_route(shape):
    body = f"<w:p><w:r><w:t>Before</w:t></w:r></w:p><w:p>{SHAPES[shape]}</w:p><w:p><w:r><w:t>After</w:t></w:r></w:p>"
    inserted = 0
    for boundary in range(9):
        doc = with_body(body)
        doc.add_bibliography_source(smith())
        after = doc.paragraphs[2].runs[0]
        before = doc.to_bytes()
        position = oxdocx.StoryRunPosition(paragraph=doc.paragraphs[1], run_index=boundary)
        try:
            doc.insert_citation(position, ["Smith20"], locale=1033)
        except oxdocx.DocxError:
            assert doc.to_bytes() == before
            continue
        inserted += 1
        assert after.text == "After"
        assert "CITATION Smith20" in reopened(doc).part_xml("/word/document.xml")
    assert inserted >= 2
