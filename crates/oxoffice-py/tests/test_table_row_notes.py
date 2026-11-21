"""Table.remove_row removes the footnotes only the removed row referenced, and a handle into a note never reads or
writes another note afterwards: it reads its own note, or raises."""

import pytest

import oxoffice.docx as oxdocx


def note_texts(doc):
    return sorted(p.text.strip() for p in doc.story_paragraphs if p.kind == "footnote")


def test_handles_into_notes_never_reach_another_note_after_a_removed_row():
    doc = oxdocx.Document()
    first = doc.add_paragraph("first")
    table = doc.add_table(2, 1)
    second = doc.add_paragraph("second")
    # A new note stales the earlier note handles, so each handle is taken as its note is made.
    handles = {}
    for paragraph, text in [(table.cell(1, 0).paragraphs[0], "in the row"), (first, "kept"), (second, "moved")]:
        handles[text] = doc.add_footnote(paragraph, text).paragraphs[0]
    assert sorted(handles) == note_texts(doc) == ["in the row", "kept", "moved"]

    table.remove_row(1)

    assert note_texts(doc) == ["kept", "moved"]
    for text, handle in handles.items():
        try:
            read = handle.text.strip()
        except oxdocx.StaleElementError:
            continue
        assert read == text, f"the handle into {text!r} reads {read!r}"
    for text, handle in handles.items():
        try:
            handle.text = text + "!"
        except oxdocx.StaleElementError:
            continue
        expected = sorted(t + "!" if t == text else t for t in ["kept", "moved"])
        assert note_texts(doc) == expected, f"writing through the handle into {text!r}"
        handle.text = text
