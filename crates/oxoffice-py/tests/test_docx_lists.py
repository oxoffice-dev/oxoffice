"""docx: list items and numbering additions. A list item is appended as a paragraph is, so it keeps the handles that
`add_paragraph` keeps, and they still read and edit their own element. A numbering addition or a list item (without
`restart=True`, which still checks every reference) checks the reference it makes: content that already names a list
the file does not define, which Word opens and shows without a number, does not refuse it and is saved as it was,
while a list item at a level its list does not have, or an instance of a missing definition, is refused without a
change."""
import io
import re
import zipfile

import docx
import pytest
import oxoffice.docx as oxdocx
from docx.oxml import OxmlElement
from docx.oxml.ns import qn

from builders import edit_cost_docx

MISSING = 42


@pytest.fixture(scope="module")
def saved(tmp_path_factory):
    return edit_cost_docx(tmp_path_factory.mktemp("lists") / "saved.docx", 10, picture_side=8)


@pytest.fixture(scope="module")
def on_missing_list(tmp_path_factory):
    """A python-docx file whose first paragraph is numbered by list 42 at level 3, a list its numbering part does not
    define."""
    document = docx.Document()
    paragraph = document.add_paragraph("no such list")
    numbering = OxmlElement("w:numPr")
    for tag, value in (("w:ilvl", 3), ("w:numId", MISSING)):
        element = OxmlElement(tag)
        element.set(qn("w:val"), str(value))
        numbering.append(element)
    paragraph._p.get_or_add_pPr().append(numbering)
    document.add_paragraph("plain")
    path = tmp_path_factory.mktemp("lists") / "missing.docx"
    document.save(path)
    return path


def new_document():
    doc = oxdocx.Document()
    doc.add_paragraph("first")
    table = doc.add_table(2, 2)
    table.cell(0, 0).text = "cell"
    doc.add_paragraph("last")
    return doc


@pytest.mark.parametrize("kind", ["new", "saved"])
def test_a_list_item_keeps_the_handles_an_appended_paragraph_keeps(saved, kind):
    def opened():
        return oxdocx.Document.open(saved) if kind == "saved" else new_document()

    def kept_after(act):
        doc = opened()
        first = doc.paragraphs[0]
        handles = {"paragraph": first, "run": first.runs[0], "cell": doc.tables[0].cell(0, 0)}
        before = {name: handle.text for name, handle in handles.items()}
        act(doc)
        kept = {}
        for name, handle in handles.items():
            try:
                kept[name] = handle.text == before[name]
            except oxdocx.StaleElementError:
                kept[name] = None
        return kept

    appended = kept_after(lambda doc: doc.add_paragraph("appended"))
    assert appended["paragraph"] and appended["cell"], appended
    assert kept_after(lambda doc: doc.add_bullet_list_item("bullet")) == appended
    assert kept_after(lambda doc: doc.add_numbered_list_item("number")) == appended

    doc = opened()
    first = doc.paragraphs[0]
    cell = doc.tables[0].cell(0, 0)
    bullet = doc.add_bullet_list_item("bullet")
    number = doc.add_numbered_list_item("number")
    assert (bullet.text, number.text) == ("bullet", "number")
    first.text = "first edited"
    cell.text = "cell edited"
    bullet.text = "bullet edited"
    reopened = oxdocx.Document.from_bytes(doc.to_bytes())
    paragraphs = reopened.paragraphs
    assert paragraphs[0].text == "first edited"
    assert reopened.tables[0].cell(0, 0).text == "cell edited"
    assert [p.text for p in paragraphs[-2:]] == ["bullet edited", "number"]
    assert [p.numbering is not None for p in paragraphs[-2:]] == [True, True]


def test_numbering_additions_and_list_items_go_on_when_content_names_a_missing_list(on_missing_list):
    doc = oxdocx.Document.open(on_missing_list)
    definition = doc.add_numbering_definition([oxdocx.ListLevel(format="decimal", start=7)])
    instance = doc.add_numbering_instance(definition)
    doc.add_bullet_list_item("bullet")
    doc.add_numbered_list_item("number")
    doc.add_paragraph("seventh").numbering = (instance, 0)

    data = doc.to_bytes()
    reopened = oxdocx.Document.from_bytes(data)
    assert [p.text for p in reopened.paragraphs] == ["no such list", "plain", "bullet", "number", "seventh"]
    assert reopened.paragraphs[0].numbering == (MISSING, 3)
    body = zipfile.ZipFile(io.BytesIO(data)).read("word/document.xml")
    assert len(re.findall(rb'<w:numId w:val="42"/>', body)) == 1


def test_a_list_item_at_a_level_its_list_does_not_have_is_refused_without_a_change():
    doc = oxdocx.Document()
    doc.add_paragraph("first")
    definition = doc.add_numbering_definition([oxdocx.ListLevel(format="bullet", text="-")])
    instance = doc.add_numbering_instance(definition)
    first = doc.paragraphs[0]
    before = doc.to_bytes()

    with pytest.raises(oxdocx.DocxError, match=f"numbering instance {instance} at missing level 4"):
        doc.add_bullet_list_item("deeper", 4)

    assert doc.to_bytes() == before
    assert first.text == "first"
    assert doc.add_bullet_list_item("top").numbering == (instance, 0)


def test_a_refused_numbering_addition_leaves_the_document_as_it_was():
    doc = oxdocx.Document()
    doc.add_paragraph("text")
    before = doc.to_bytes()
    with pytest.raises(oxdocx.DocxError, match="numbering definition 99 does not exist"):
        doc.add_numbering_instance(99)
    assert doc.to_bytes() == before
