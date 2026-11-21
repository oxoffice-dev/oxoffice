"""Inserting another document keeps the names of its bookmarks while they are free in the destination."""

import oxoffice.docx as oxdocx


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
