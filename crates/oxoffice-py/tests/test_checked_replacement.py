"""Document.replace_all refuses a pair that leaves its text out of its replacement's reach, names the paragraphs
that hold it and changes nothing, and replaces what it can reach with allow_unreached=True, on the Word and Google
Docs files of the corpus."""

import io
import zipfile
from pathlib import Path

import pytest

import oxoffice.docx as oxdocx

CORPUS = Path(__file__).resolve().parents[3] / "tests" / "corpus" / "files"
PRODUCERS = ["word", "google"]

# ACME once in a plain paragraph, then cut by the edge of a content control, of a tracked insertion and of a field.
SPLIT = (
    '<w:p><w:r><w:t>ACME plain</w:t></w:r></w:p>'
    '<w:p><w:r><w:t>AC</w:t></w:r><w:sdt><w:sdtContent><w:r><w:t>ME control</w:t></w:r></w:sdtContent></w:sdt></w:p>'
    '<w:p><w:r><w:t>AC</w:t></w:r><w:ins w:id="901" w:author="a" w:date="2026-01-01T00:00:00Z">'
    '<w:r><w:t>ME insertion</w:t></w:r></w:ins></w:p>'
    '<w:p><w:r><w:t>AC</w:t></w:r><w:fldSimple w:instr=" AUTHOR "><w:r><w:t>ME field</w:t></w:r></w:fldSimple></w:p>'
)


def report(producer, extra=SPLIT):
    """The corpus report of `producer` with `extra` before the last section properties of its body."""
    data = (CORPUS / producer / "report.docx").read_bytes()
    out = io.BytesIO()
    with zipfile.ZipFile(io.BytesIO(data)) as source, zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as target:
        for item in source.infolist():
            part = source.read(item.filename)
            if item.filename == "word/document.xml":
                xml = part.decode("utf-8")
                at = xml.rfind("<w:sectPr")
                part = (xml[:at] + extra + xml[at:]).encode("utf-8")
            target.writestr(item, part)
    return oxdocx.Document.from_bytes(out.getvalue())


def read_once(doc):
    """Read the document's stories, which moves later edits to the path that keeps live snapshots current."""
    assert doc.story_paragraphs
    return doc


@pytest.fixture(params=[(producer, read) for producer in PRODUCERS for read in (False, True)],
                ids=lambda param: f"{param[0]}-{'read' if param[1] else 'fresh'}")
def doc(request):
    producer, read = request.param
    document = report(producer)
    return read_once(document) if read else document


def test_a_pair_cut_by_a_control_a_revision_or_a_field_is_refused_and_nothing_changes(doc):
    before = doc.to_bytes()
    with pytest.raises(oxdocx.UnreachedTextError) as refused:
        doc.replace_all([("Northwind", "Southwind", 1), ("ACME", "Globex", None)])
    error = refused.value
    assert isinstance(error, oxdocx.DocxError)
    assert (error.index, error.placeholder, error.left) == (1, "ACME", 3)
    assert [p.text for p in error.paragraphs] == ["ACME control", "ACME insertion", "ACME field"]
    assert {(p.kind, p.part_name) for p in error.paragraphs} == {("body", "/word/document.xml")}
    assert 'pair 1: 3 occurrence(s) of "ACME" left after its replacement' in str(error)
    assert "allow_unreached=True" in str(error)
    assert doc.to_bytes() == before
    assert doc.count_text("Northwind") == {"header": 1}


def test_allow_unreached_replaces_what_can_be_reached(doc):
    assert doc.replace_all([("ACME", "Globex", 1)], allow_unreached=True) == (1,)
    assert doc.count_text("ACME") == {"body": 3}
    assert doc.count_text("Globex plain") == {"body": 1}


def test_a_count_mismatch_is_reported_before_the_reach(doc):
    with pytest.raises(oxdocx.ReplacementCountError) as refused:
        doc.replace_all([("ACME", "Globex", 4)])
    assert (refused.value.expected, refused.value.found, refused.value.index) == (4, 1, 0)


def test_a_replacement_that_holds_its_text_names_the_paragraphs_it_left_unchanged(doc):
    with pytest.raises(oxdocx.UnreachedTextError) as refused:
        doc.replace_all([("ACME", "ACME Corp")])
    assert refused.value.left == 3
    assert [p.text for p in refused.value.paragraphs] == ["ACME control", "ACME insertion", "ACME field"]


@pytest.mark.parametrize("producer", PRODUCERS)
def test_a_document_the_check_could_not_read_does_not_open(producer):
    """The Rust check refuses a document whose header part is missing (ReplacementRefusal::Unchecked); Python
    refuses to open it at all, so replace_all never meets it."""
    data = (CORPUS / producer / "report.docx").read_bytes()
    out = io.BytesIO()
    with zipfile.ZipFile(io.BytesIO(data)) as source, zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as target:
        for item in source.infolist():
            if item.filename != "word/header1.xml":
                target.writestr(item, source.read(item.filename))
    with pytest.raises(oxdocx.DocxError, match="missing part"):
        oxdocx.Document.from_bytes(out.getvalue())


@pytest.mark.parametrize("producer", PRODUCERS)
def test_reachable_pairs_replace_as_before(producer):
    doc = report(producer, "")
    assert doc.replace_all([("Northwind", "Southwind", 1), ("finance", "accounting")]) == (1, 0)
    assert doc.count_text("finance") == {"comment": 1}, "comments are not searched, and do not refuse"


@pytest.mark.parametrize("producer", PRODUCERS)
def test_handles_read_after_a_replacement_made_before_any_read_as_after_one_made_after(producer):
    """A replacement made before any read builds no owner facts of the result: the first read after it does, and its
    handles read and edit as those of a document read before its replacement."""
    fresh, read = report(producer, ""), read_once(report(producer, ""))
    for doc in (fresh, read):
        assert doc.replace_all([("Northwind", "Southwind", 1), ("Revenue", "Turnover")]) == (1, 2)
        assert doc.try_replace_text("Highlights", "Key points") == 1
        doc.paragraphs[1].add_run(" (edited)")
        doc.tables[0].cell(0, 0).text = "edited cell"
    assert [p.text for p in fresh.paragraphs] == [p.text for p in read.paragraphs]
    assert fresh.paragraphs[1].text.startswith("Summary. Turnover grew")
    assert fresh.to_bytes() == read.to_bytes()


@pytest.mark.parametrize("producer", PRODUCERS)
def test_try_replace_text_does_not_check_the_reach(producer):
    doc = report(producer)
    assert doc.try_replace_text("ACME", "Globex") == 1
    assert doc.count_text("ACME") == {"body": 3}
