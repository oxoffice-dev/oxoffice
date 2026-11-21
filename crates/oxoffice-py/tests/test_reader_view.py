"""What a user reads shows the reader's view: a simple field's result and the text of a tracked insertion, in the text,
the Markdown and the HTML, and in a comment."""

import io
import zipfile

import oxoffice.docx as oxdocx

INSERTED = (
    '<w:r><w:t xml:space="preserve">A </w:t></w:r>'
    '<w:ins w:id="905" w:author="a" w:date="2026-01-01T00:00:00Z">'
    '<w:fldSimple w:instr=" QUOTE a "><w:r><w:t>12</w:t></w:r></w:fldSimple>'
    "<w:r><w:t>C</w:t></w:r></w:ins>"
)
CHAPTER = (
    '<w:r><w:t xml:space="preserve">Chapter </w:t></w:r>'
    '<w:fldSimple w:instr=" SEQ Chapter "><w:r><w:t>1</w:t></w:r></w:fldSimple>'
)


def rewritten(doc, part, edit):
    source = zipfile.ZipFile(io.BytesIO(doc.to_bytes()))
    out = io.BytesIO()
    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as target:
        for item in source.infolist():
            data = source.read(item.filename)
            if item.filename == part:
                data = edit(data.decode("utf-8")).encode("utf-8")
            target.writestr(item, data)
    return oxdocx.Document.from_bytes(out.getvalue())


def test_a_tracked_insertion_holding_a_simple_field_reads_with_its_result():
    doc = oxdocx.Document()
    doc.add_paragraph("MARKER")
    doc = rewritten(doc, "word/document.xml",
                    lambda xml: xml.replace("<w:r><w:t>MARKER</w:t></w:r>", INSERTED, 1))
    assert doc.paragraphs[0].text == "A 12C"
    assert doc.to_markdown().strip() == "A 12C"
    assert "12" in doc.to_html()


def test_a_comment_text_shows_its_simple_field_results():
    doc = oxdocx.Document()
    doc.add_paragraph("Body")
    doc.add_comment(oxdocx.RunRange(start=oxdocx.RunPosition(body_index=0, run_index=0),
                                    end=oxdocx.RunPosition(body_index=0, run_index=1)), author="Ada", text="MARKER")
    doc = rewritten(doc, "word/comments.xml", lambda xml: xml.replace("<w:r><w:t>MARKER</w:t></w:r>", CHAPTER, 1))
    assert [comment.text for comment in doc.comments] == ["Chapter 1"]
