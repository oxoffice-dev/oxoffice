"""Inserting another document keeps the names of its bookmarks while they are free in the destination."""

from pathlib import Path

import oxoffice.docx as oxdocx

CORPUS = Path(__file__).resolve().parents[3] / "tests" / "corpus" / "files"


def bookmarked(name):
    doc = oxdocx.Document()
    doc.add_paragraph("bookmarked text")
    index = len(doc.paragraphs) - 1
    doc.add_bookmark(name, oxdocx.RunRange(start=oxdocx.RunPosition(body_index=index, run_index=0),
                                           end=oxdocx.RunPosition(body_index=index, run_index=1)))
    return doc


def names(doc):
    return sorted(bookmark.name for bookmark in oxdocx.Document.from_bytes(doc.to_bytes()).bookmarks)


def test_an_inserted_document_keeps_a_free_bookmark_name_and_renames_a_taken_one():
    doc = bookmarked("Intro")
    doc.insert_document(bookmarked("Summary"), at=0)
    assert names(doc) == ["Intro", "Summary"]
    doc.insert_document(bookmarked("SUMMARY"), at=0)
    after = names(doc)
    assert len(after) == 3 and "SUMMARY" not in after, after


def accent_document(run_property, text):
    import io
    import zipfile
    doc = oxdocx.Document()
    doc.add_paragraph(text)
    source = zipfile.ZipFile(io.BytesIO(doc.to_bytes()))
    output = io.BytesIO()
    with zipfile.ZipFile(output, "w", zipfile.ZIP_DEFLATED) as target:
        for item in source.infolist():
            data = source.read(item.filename)
            if item.filename == "word/styles.xml":
                style = (f'<w:style w:type="paragraph" w:styleId="Accent"><w:name w:val="Accent"/>'
                         f'<w:rPr>{run_property}</w:rPr></w:style>').encode()
                data = data.replace(b"</w:styles>", style + b"</w:styles>")
            if item.filename == "word/document.xml":
                data = data.replace(b"<w:p>", b'<w:p><w:pPr><w:pStyle w:val="Accent"/></w:pPr>', 1)
            target.writestr(item, data)
    return oxdocx.Document.from_bytes(output.getvalue())


def test_insert_document_can_keep_the_destination_formatting():
    for keep, expected in ((True, "Accent"), (False, None)):
        doc = accent_document("<w:b/>", "destination")
        policy = oxdocx.FragmentConflictPolicy.reuse_equivalent().with_destination_formatting(keep)
        doc.insert_document(accent_document("<w:i/>", "imported"), at=1, policy=policy)
        saved = oxdocx.Document.from_bytes(doc.to_bytes())
        imported = next(p for p in saved.paragraphs if p.text == "imported")
        if expected:
            assert imported.style == expected
        else:
            assert imported.style != "Accent" and imported.style.startswith("Accent")


def test_insert_document_takes_the_destination_s_styles_by_default():
    doc = accent_document("<w:b/>", "destination")
    doc.insert_document(accent_document("<w:i/>", "imported"), at=1)
    saved = oxdocx.Document.from_bytes(doc.to_bytes())
    imported = next(p for p in saved.paragraphs if p.text == "imported")
    assert imported.style == "Accent"


def test_an_inserted_document_brings_the_styles_its_headers_and_footers_use():
    doc = oxdocx.Document.from_bytes((CORPUS / "google" / "report.docx").read_bytes())
    source = oxdocx.Document.from_bytes((CORPUS / "libreoffice" / "report.docx").read_bytes())
    doc.insert_document(source, at=0)
    saved = oxdocx.Document.from_bytes(doc.to_bytes())
    undefined = [finding.message for finding in saved.validate().findings if "is not defined" in finding.message]
    assert undefined == []
