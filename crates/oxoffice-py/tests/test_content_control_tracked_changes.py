"""A content control's value replaces its content with the tracked changes in it, in Python as in the Rust facade, as
an edit made with track changes off. One holding the mark of a comment is refused and the document left as it was."""

import io
import zipfile

import pytest

import oxoffice.docx as oxdocx

TRACKED = 'w:author="Ada" w:date="2026-11-24T09:00:00Z"'


def with_body(body):
    """A new document whose body is `body`."""
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


def inline_control(content):
    """A paragraph holding "A " then an inline control tagged "t" holding `content`."""
    return (
        '<w:p><w:r><w:t xml:space="preserve">A </w:t></w:r><w:sdt><w:sdtPr><w:tag w:val="t"/><w:id w:val="1"/>'
        f"</w:sdtPr><w:sdtContent>{content}</w:sdtContent></w:sdt></w:p>"
    )


def body_xml(doc):
    with zipfile.ZipFile(io.BytesIO(doc.to_bytes())) as package:
        return package.read("word/document.xml").decode("utf-8")


def test_a_value_replaces_the_tracked_insertion_and_deletion_of_a_control():
    content = (
        f'<w:r><w:t>B</w:t></w:r><w:ins w:id="5" {TRACKED}><w:r><w:t>C</w:t></w:r></w:ins>'
        f'<w:del w:id="6" {TRACKED}><w:r><w:delText>Z</w:delText></w:r></w:del>'
    )
    doc = with_body(inline_control(content))
    assert doc.set_content_control_value_by_tag("t", "X") == 1
    doc = oxdocx.Document.from_bytes(doc.to_bytes())
    assert doc.paragraphs[0].text == "A X"
    assert [control.text for control in doc.content_controls()] == ["X"]
    xml = body_xml(doc)
    assert "<w:ins " not in xml and "<w:del " not in xml, xml


def test_a_value_over_a_tracked_change_holding_a_comment_mark_is_refused():
    content = (
        '<w:commentRangeStart w:id="0"/><w:r><w:t>B</w:t></w:r><w:commentRangeEnd w:id="0"/>'
        f'<w:ins w:id="5" {TRACKED}><w:r><w:t>C</w:t></w:r><w:r><w:commentReference w:id="0"/></w:r></w:ins>'
    )
    doc = with_body(inline_control(content))
    before = body_xml(doc)
    with pytest.raises(oxdocx.DocxError, match="comment 0.*accept or reject it first"):
        doc.set_content_control_value_by_tag("t", "X")
    assert [control.text for control in doc.content_controls()] == ["BC"]
    assert body_xml(doc) == before
