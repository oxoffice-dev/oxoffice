"""docx: a mail-merge package lives in the settings, a custom XML store and a recipient part, none of which a handle
names, so writing or removing it keeps every handle. The edit is made in place, without saving the document: a refused
edit, and an edit that changes nothing, leave the document as it was, byte for byte."""
import pytest

import oxoffice.docx as oxdocx
from builders import edit_cost_docx
from test_docx_comment_edits import before, handles_taken, reads

SOURCE = oxdocx.MailMergePackageSource


def package(*records, recipients=()):
    rows = [[record] for record in records] or [["first"]]
    return oxdocx.MailMergePackageOptions(SOURCE.embedded(["name"], rows), recipients=recipients)


@pytest.fixture(scope="module")
def saved(tmp_path_factory):
    """The document of test_docx_comment_edits: a body with tables, two sections, a header and a comment."""
    path = edit_cost_docx(tmp_path_factory.mktemp("merge") / "plain.docx", 12)
    doc = oxdocx.Document.open(path)
    doc.add_comment_on_text(doc.paragraphs[5].runs[0].text, author="Reviewer", text="a remark")
    out = path.with_name("saved.docx")
    out.write_bytes(doc.to_bytes())
    return out


def records(doc):
    return doc.mail_merge_package().source.records


@pytest.mark.parametrize("source", [package("first", "second"), oxdocx.MailMergePackageOptions(
    SOURCE.external("https://example.com/recipients.csv"))], ids=["embedded", "external"])
def test_writing_a_package_keeps_every_handle(saved, source):
    doc = oxdocx.Document.open(saved)
    taken = handles_taken(doc)
    doc.set_mail_merge_package(source)
    assert reads(taken) == before(taken)
    # The handles still edit their element.
    taken["paragraph"][0].text = "edited paragraph"
    taken["cell"][0].text = "edited cell"
    again = oxdocx.Document.from_bytes(doc.to_bytes())
    assert again.mail_merge_package().source.kind == source.source.kind
    assert again.paragraphs[3].text == "edited paragraph"
    assert again.tables[0].cell(0, 1).text == "edited cell"


def test_rewriting_and_removing_a_package_keep_every_handle(saved):
    doc = oxdocx.Document.open(saved)
    doc.set_mail_merge_package(package("first"))
    taken = handles_taken(doc)
    doc.set_mail_merge_package(package("second", "third"))
    assert records(doc) == (("second",), ("third",))
    assert reads(taken) == before(taken)
    assert doc.remove_mail_merge_package() is True
    assert doc.mail_merge_package() is None
    assert reads(taken) == before(taken)
    again = oxdocx.Document.from_bytes(doc.to_bytes())
    assert again.mail_merge_package() is None
    # A store and a recipient part written again after the removal take the names the removal freed.
    doc.set_mail_merge_package(package("fourth", recipients=[oxdocx.MailMergeRecipient(0, b"tag", active=False)]))
    names = set(oxdocx.Document.from_bytes(doc.to_bytes()).part_names())
    assert "/customXml/item1.xml" in names and "/word/recipientData.xml" in names


def test_writing_the_same_package_twice_reads_the_same_document(saved):
    doc = oxdocx.Document.open(saved)
    doc.set_mail_merge_package(package("first", "second"))
    once = doc.to_bytes()
    taken = handles_taken(doc)
    doc.set_mail_merge_package(package("first", "second"))
    assert doc.to_bytes() == once
    assert reads(taken) == before(taken)


def test_removing_an_absent_package_leaves_the_document_as_it_was(saved):
    doc = oxdocx.Document.open(saved)
    unchanged = doc.to_bytes()
    taken = handles_taken(doc)
    assert doc.remove_mail_merge_package() is False
    assert doc.to_bytes() == unchanged
    doc.set_mail_merge_package(package("first"))
    assert doc.remove_mail_merge_package() is True
    removed = doc.to_bytes()
    assert doc.remove_mail_merge_package() is False
    assert doc.to_bytes() == removed
    assert reads(taken) == before(taken)


@pytest.mark.parametrize("refused", [
    lambda: oxdocx.MailMergePackageOptions(SOURCE.external("relative/recipients.csv")),
    lambda: oxdocx.MailMergePackageOptions(SOURCE.embedded(["name", "name"], [["a", "b"]])),
], ids=["relative uri", "duplicate column"])
def test_a_refused_package_leaves_the_document_as_it_was(saved, refused):
    doc = oxdocx.Document.open(saved)
    doc.set_mail_merge_package(package("first"))
    unchanged = doc.to_bytes()
    taken = handles_taken(doc)
    with pytest.raises(oxdocx.DocxError):
        doc.set_mail_merge_package(refused())
    assert doc.to_bytes() == unchanged
    assert records(doc) == (("first",),)
    assert reads(taken) == before(taken)


def test_protecting_a_form_keeps_every_handle(saved):
    doc = oxdocx.Document.open(saved)
    taken = handles_taken(doc)
    doc.protect_form([], oxdocx.DocumentProtection(mode="forms", enforcement=True))
    assert doc.document_protection.mode == "forms"
    assert reads(taken) == before(taken)
    with pytest.raises(oxdocx.DocxError):
        doc.protect_form([], oxdocx.DocumentProtection(mode="readOnly", enforcement=True))
    assert doc.document_protection.mode == "forms"
