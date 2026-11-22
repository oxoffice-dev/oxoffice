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


def google_heading_document():
    """A body whose heading has its bookmark start before it, as Google Docs writes it."""
    import io
    import zipfile
    doc = oxdocx.Document()
    for text in ("kept", "Heading", "end"):
        doc.add_paragraph(text)
    source = zipfile.ZipFile(io.BytesIO(doc.to_bytes()))
    output = io.BytesIO()
    with zipfile.ZipFile(output, "w", zipfile.ZIP_DEFLATED) as target:
        for item in source.infolist():
            data = source.read(item.filename)
            if item.filename == "word/document.xml":
                heading = data.index(b"Heading")
                start = data.rindex(b"<w:p>", 0, heading)
                end = data.index(b"</w:p>", heading)
                data = (data[:start] + b'<w:bookmarkStart w:id="7" w:name="_heading"/>' + data[start:end]
                        + b'<w:bookmarkEnd w:id="7"/>' + data[end:])
            target.writestr(item, data)
    return oxdocx.Document.from_bytes(output.getvalue())


def test_a_popped_heading_pasted_twice_keeps_its_bookmark_whole_then_takes_a_fresh_one():
    doc = google_heading_document()
    heading = next(item for item in doc.story_items if item.text == "Heading")
    fragment = doc.pop_content(heading)

    def body():
        return next(story for story in doc.stories if story.kind == "body")

    doc.insert_content(body(), fragment)
    saved = oxdocx.Document.from_bytes(doc.to_bytes())
    assert [bookmark.name for bookmark in saved.bookmarks] == ["_heading"]
    doc.insert_content(body(), fragment)
    saved = oxdocx.Document.from_bytes(doc.to_bytes())
    names = [bookmark.name for bookmark in saved.bookmarks]
    assert len(names) == 2 and names[0] == "_heading" and names[1] != "_heading", names
    assert [p.text for p in saved.paragraphs] == ["kept", "end", "Heading", "Heading"]


def test_a_removed_plain_paragraph_takes_the_bookmark_around_it():
    import io
    import zipfile
    doc = oxdocx.Document()
    for text in ("kept", "plain", "end"):
        doc.add_paragraph(text)
    source = zipfile.ZipFile(io.BytesIO(doc.to_bytes()))
    output = io.BytesIO()
    with zipfile.ZipFile(output, "w", zipfile.ZIP_DEFLATED) as target:
        for item in source.infolist():
            data = source.read(item.filename)
            if item.filename == "word/document.xml":
                plain = data.index(b"plain")
                start = data.rindex(b"<w:p>", 0, plain)
                end = data.index(b"</w:p>", plain) + len(b"</w:p>")
                data = (data[:start] + b'<w:bookmarkStart w:id="3" w:name="Around"/>' + data[start:end]
                        + b'<w:bookmarkEnd w:id="3"/>' + data[end:])
            target.writestr(item, data)
    doc = oxdocx.Document.from_bytes(output.getvalue())
    assert [bookmark.name for bookmark in doc.bookmarks] == ["Around"]
    index = next(i for i, item in enumerate(doc.story_items) if item.text == "plain")
    assert doc.remove_content(index)
    saved = oxdocx.Document.from_bytes(doc.to_bytes())
    assert [p.text for p in saved.paragraphs] == ["kept", "end"]
    assert list(saved.bookmarks) == []
    plain = oxdocx.Document()
    for text in ("kept", "end"):
        plain.add_paragraph(text)
    plain = oxdocx.Document.from_bytes(plain.to_bytes())
    assert [item.kind for item in saved.story_items] == [item.kind for item in plain.story_items]
