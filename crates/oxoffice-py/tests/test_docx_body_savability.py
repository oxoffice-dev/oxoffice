"""docx: a body that a typed edit cannot leave savable (here, one whose w:body declares a namespace) refuses the typed
inserts before they change it, every time; the other edits leave the refusal to the save. A body that can be saved
after a typed insert accepts one insert after another, including one that keeps nested namespace owners from its
source (its w15 repeating section), on which the answer kept between inserts depends."""
import re
import zipfile

import pytest
import oxoffice.docx as oxdocx

from builders import body_namespace_docx, edit_cost_docx

INSERTS = {
    "Document.add_paragraph": lambda doc: doc.add_paragraph("added"),
    "Document.add_heading": lambda doc: doc.add_heading("heading", 1),
    "Document.add_page_break": lambda doc: doc.add_page_break(),
    "Document.add_table": lambda doc: doc.add_table(1, 1),
    "Document.insert_paragraph": lambda doc: doc.insert_paragraph(0, "inserted"),
    "Document.insert_table": lambda doc: doc.insert_table(0, 1, 1),
    "Paragraph.insert_paragraph_before": lambda doc: doc.paragraphs[0].insert_paragraph_before("before"),
    "Document.add_bullet_list_item": lambda doc: doc.add_bullet_list_item("bullet"),
    "Document.add_numbered_list_item": lambda doc: doc.add_numbered_list_item("number"),
}


@pytest.fixture(scope="module")
def namespaced(tmp_path_factory):
    return body_namespace_docx(tmp_path_factory.mktemp("savability") / "namespaced.docx")


@pytest.fixture(scope="module")
def saved(tmp_path_factory):
    return edit_cost_docx(tmp_path_factory.mktemp("savability") / "saved.docx", 10, picture_side=8)


@pytest.mark.parametrize("name", sorted(INSERTS))
def test_a_typed_insert_is_refused_again_on_a_body_it_cannot_leave_savable(namespaced, name):
    doc = oxdocx.Document.open(namespaced)
    before = doc.to_bytes()
    for _ in range(2):
        with pytest.raises(oxdocx.DocxError, match="shadowed `x` namespace"):
            INSERTS[name](doc)
    assert doc.to_bytes() == before


def test_an_edit_that_is_not_an_insert_is_refused_by_the_save(namespaced):
    doc = oxdocx.Document.open(namespaced)
    doc.paragraphs[0].text = "edited"
    with pytest.raises(oxdocx.DocxError, match="shadowed `x` namespace"):
        doc.to_bytes()


def test_typed_inserts_follow_one_another_on_a_savable_body(saved):
    body = zipfile.ZipFile(saved).read("word/document.xml").split(b"<w:body>")[1]
    assert re.search(rb"<w:sdt [^>]*xmlns:w15=", body), "the body keeps a nested namespace owner (a w15 control)"
    doc = oxdocx.Document.open(saved)
    for name in sorted(INSERTS):
        INSERTS[name](doc)
    reopened = oxdocx.Document.from_bytes(doc.to_bytes())
    texts = [p.text for p in reopened.paragraphs]
    assert {"added", "heading", "inserted", "before"} <= set(texts)
    assert len(reopened.tables) == len(oxdocx.Document.open(saved).tables) + 2
