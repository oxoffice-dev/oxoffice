"""FootnoteEndnote.add_paragraph and add_table edit the note in place: the body is neither saved nor reopened, the
handles outside the note keep working, and the document saves what the handles read."""

import pytest

import oxoffice.docx as oxdocx
from builders import edit_cost_docx
from test_docx_block_handles import STALE, outcome
from test_docx_comment_edits import before, handles_taken, reads


@pytest.fixture(scope="module")
def saved(tmp_path_factory):
    """The edit-cost document with a footnote on paragraph 2 and an endnote on paragraph 4."""
    path = edit_cost_docx(tmp_path_factory.mktemp("notes") / "plain.docx", 12)
    doc = oxdocx.Document.open(path)
    doc.add_footnote(doc.paragraphs[2], "other footnote")
    doc.add_endnote(doc.paragraphs[4], "other endnote")
    out = path.with_name("saved.docx")
    out.write_bytes(doc.to_bytes())
    return out


def add_note(doc, kind, text, at=6):
    """A new note on paragraph `at`: a note handle is taken as its note is made, a new note staling the others."""
    add = doc.add_footnote if kind == "footnote" else doc.add_endnote
    return add(doc.paragraphs[at], text)


def note_paragraphs(doc, kind):
    return [p for p in doc.story_paragraphs if p.kind == kind]


def note_texts(doc, kind):
    """The texts of the note paragraphs, the empty ones of a table in a note aside."""
    return [p.text.strip() for p in note_paragraphs(doc, kind) if p.text.strip()]


@pytest.mark.parametrize("kind", ["footnote", "endnote"])
def test_adding_a_paragraph_to_a_note_keeps_the_handles_outside_it(saved, kind):
    doc = oxdocx.Document.open(saved)
    target = add_note(doc, kind, "new note")
    taken = handles_taken(doc)
    (other,) = [p for p in note_paragraphs(doc, kind) if p.text.strip().startswith("other")]
    (inside,) = target.paragraphs

    added = target.add_paragraph("more")

    assert added.text == "more"
    assert note_texts(doc, kind) == [f"other {kind}", "new note", "more"]
    assert reads(taken) == before(taken)
    assert other.text.strip() == f"other {kind}"
    # The note's own story is published anew: an earlier handle into it retires, the note handle follows.
    assert outcome(lambda: inside.text) == STALE
    assert [p.text.strip() for p in target.paragraphs] == ["new note", "more"]
    # The handles kept still edit their element, and the save holds every edit.
    taken["paragraph"][0].text = "edited paragraph"
    added.text = "edited note"
    again = oxdocx.Document.from_bytes(doc.to_bytes())
    assert again.paragraphs[3].text == "edited paragraph"
    assert note_texts(again, kind) == [f"other {kind}", "new note", "edited note"]


@pytest.mark.parametrize("kind", ["footnote", "endnote"])
def test_adding_a_table_to_a_note_keeps_the_handles_outside_it(saved, kind):
    doc = oxdocx.Document.open(saved)
    target = add_note(doc, kind, "new note")
    taken = handles_taken(doc)

    table = target.add_table(2, 3)

    assert reads(taken) == before(taken)
    (read,) = target.tables
    assert [len(row.cells) for row in read.rows] == [3, 3]
    table.cell(1, 2).text = "in the note"
    again = oxdocx.Document.from_bytes(doc.to_bytes())
    assert note_texts(again, kind) == [f"other {kind}", "new note", "in the note"]


def test_a_note_edited_three_times_saves_its_blocks_in_order(saved):
    doc = oxdocx.Document.open(saved)
    target = add_note(doc, "footnote", "new note")
    target.add_paragraph("one")
    table = target.add_table(1, 1)
    target.add_paragraph("two")
    # The table's own story was published anew by the paragraph after it.
    assert outcome(lambda: table.cell(0, 0).text) == STALE
    target.tables[0].cell(0, 0).text = "cell"
    again = oxdocx.Document.from_bytes(doc.to_bytes())
    assert note_texts(again, "footnote") == ["other footnote", "new note", "one", "cell", "two"]
    texts = [p.text for p in again.story_paragraphs]
    assert texts.index("one") < texts.index("cell") < texts.index("two")


def test_a_table_in_a_note_survives_a_new_note_and_a_reopen(saved):
    doc = oxdocx.Document.open(saved)
    add_note(doc, "footnote", "new note").add_table(1, 2).cell(0, 1).text = "kept cell"
    add_note(doc, "footnote", "later note", at=8).add_paragraph("after")
    again = oxdocx.Document.from_bytes(doc.to_bytes())
    assert "kept cell" in [p.text for p in again.story_paragraphs]
    assert note_texts(again, "footnote") == ["other footnote", "new note", "kept cell", "later note", "after"]



def add_to(doc, kind):
    return doc.add_footnote if kind == "footnote" else doc.add_endnote


@pytest.mark.parametrize("kind", ["footnote", "endnote"])
@pytest.mark.parametrize("reference", ["paragraph", "run"])
def test_a_note_on_a_body_paragraph_keeps_every_other_handle(saved, kind, reference):
    doc = oxdocx.Document.open(saved)
    taken = handles_taken(doc)
    edited = doc.paragraphs[6]
    text = edited.text
    edited_run = edited.runs[1]
    (other,) = [p for p in note_paragraphs(doc, kind) if p.text.strip() == f"other {kind}"]

    note = add_to(doc, kind)(edited if reference == "paragraph" else edited_run, "new note")

    assert [p.text.strip() for p in note.paragraphs] == ["new note"]
    assert note_texts(doc, kind) == [f"other {kind}", "new note"]
    # The paragraph and note edited alone: every handle outside them reads what it read.
    assert reads(taken) == before(taken)
    assert other.text.strip() == f"other {kind}"
    # The edited paragraph keeps its handle; its runs are born again, as any edit of the paragraph makes them.
    assert edited.text == text
    assert outcome(lambda: edited_run.text) == STALE
    # The reference goes after the run, or at the paragraph's end, and the kept handles still edit.
    reference_at = 2 if reference == "run" else 6
    assert f"w:{kind}Reference" in edited.runs[reference_at].xml
    taken["run"][0].text = "edited run"
    note.add_paragraph("more")
    again = oxdocx.Document.from_bytes(doc.to_bytes())
    assert again.paragraphs[3].runs[2].text == "edited run"
    assert f"w:{kind}Reference" in again.paragraphs[6].runs[reference_at].xml
    assert note_texts(again, kind) == [f"other {kind}", "new note", "more"]


@pytest.mark.parametrize("kind", ["footnote", "endnote"])
def test_a_refused_note_leaves_the_document_and_every_handle_as_they_were(saved, kind):
    doc = oxdocx.Document.open(saved)
    taken = handles_taken(doc)
    edited = doc.paragraphs[6]
    edited_run = edited.runs[1]
    data = doc.to_bytes()

    with pytest.raises(ValueError, match="cannot carry"):
        add_to(doc, kind)(edited_run, "bad \x01 text")

    assert doc.to_bytes() == data
    assert reads(taken) == before(taken)
    assert edited_run.text == "run 1 of paragraph 6 "


@pytest.mark.parametrize("kind", ["footnote", "endnote"])
def test_a_note_in_a_table_cell_is_added_and_saved(saved, kind):
    doc = oxdocx.Document.open(saved)
    cell = doc.tables[0].cell(0, 0)

    note = add_to(doc, kind)(cell.paragraphs[0], "cell note")

    assert [p.text.strip() for p in note.paragraphs] == ["cell note"]
    assert f"w:{kind}Reference" in cell.paragraphs[0].xml
    again = oxdocx.Document.from_bytes(doc.to_bytes())
    assert note_texts(again, kind) == [f"other {kind}", "cell note"]
    assert f"w:{kind}Reference" in again.tables[0].cell(0, 0).paragraphs[0].xml
