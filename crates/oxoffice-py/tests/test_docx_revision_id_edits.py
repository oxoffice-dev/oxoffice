"""Accepting or rejecting one revision id: resolved in its body paragraph in place, every other handle kept."""

import re
import zipfile

import pytest

import oxoffice.docx as oxdocx
from builders import edit_cost_docx
from test_docx_block_handles import STALE, outcome


@pytest.fixture(scope="module")
def saved(tmp_path_factory):
    """A body with paragraphs "tracked k" holding a tracked insertion of id 901 + k, a table, a header and a comment
    on the fifth paragraph."""
    path = edit_cost_docx(tmp_path_factory.mktemp("revisions") / "plain.docx", 12)
    doc = oxdocx.Document.open(path)
    doc.add_comment_on_text(doc.paragraphs[5].runs[0].text, author="Reviewer", text="a remark")
    out = path.with_name("saved.docx")
    out.write_bytes(doc.to_bytes())
    return out


def handles_taken(doc):
    """A handle of each kind the binding tracks, each with how to read it and what it reads now."""
    table = doc.tables[0]
    (comment,) = [comment for comment in doc.comments if comment.text == "a remark"]
    taken = {
        "paragraph": (doc.paragraphs[3], lambda h: h.text),
        "run": (doc.paragraphs[3].runs[2], lambda h: h.text),
        "table": (table, lambda h: h.cell(1, 1).text),
        "cell": (table.cell(0, 1), lambda h: h.text),
        "comment": (comment, lambda h: (h.text, h.anchor_text)),
        "header paragraph": (doc.section(0).header.paragraphs[0], lambda h: h.text),
        "section": (doc.section(0), lambda h: h.page_width),
    }
    return {name: (handle, read, read(handle)) for name, (handle, read) in taken.items()}


def reads(taken):
    return {name: outcome(lambda: read(handle)) for name, (handle, read, _) in taken.items()}


def before(taken):
    return {name: value for name, (_, _, value) in taken.items()}


def tracked(doc, k):
    (paragraph,) = [paragraph for paragraph in doc.paragraphs if paragraph.text.startswith(f"tracked {k} ")]
    return paragraph


@pytest.mark.parametrize(("accept", "text"), [(True, "tracked 1 inserted 1"), (False, "tracked 1 ")])
def test_one_revision_id_keeps_every_handle_outside_its_paragraph(saved, accept, text):
    doc = oxdocx.Document.open(saved)
    taken = handles_taken(doc)
    edited = tracked(doc, 1)
    xml = edited.xml

    count = (doc.accept_revision_id if accept else doc.reject_revision_id)(902)

    assert count == 1
    assert reads(taken) == before(taken)
    # The edited paragraph keeps its handle and reads the resolution.
    assert edited.text == text
    assert "w:ins" in xml and "w:ins" not in edited.xml
    taken["run"][0].text = "edited run"
    taken["cell"][0].text = "edited cell"
    again = oxdocx.Document.from_bytes(doc.to_bytes())
    assert again.paragraphs[3].runs[2].text == "edited run"
    assert again.tables[0].cell(0, 1).text == "edited cell"
    assert tracked(again, 1).text == text
    assert "w:ins" not in tracked(again, 1).xml
    assert "w:ins" in tracked(again, 0).xml


@pytest.mark.parametrize("accept", [True, False])
def test_an_unknown_revision_id_resolves_nothing_and_keeps_every_handle(saved, accept):
    doc = oxdocx.Document.open(saved)
    taken = handles_taken(doc)
    run = tracked(doc, 0).runs[0]
    data = doc.to_bytes()

    assert (doc.accept_revision_id if accept else doc.reject_revision_id)(4242) == 0

    assert doc.to_bytes() == data
    assert reads(taken) == before(taken)
    assert outcome(lambda: run.text) != STALE


def with_header_insertion(source, target):
    """`source` with a tracked insertion of id 950 at the end of the first header paragraph."""
    with zipfile.ZipFile(source) as z:
        parts = {name: z.read(name) for name in z.namelist()}
    (header,) = [name for name in parts if re.fullmatch(r"word/header\d*\.xml", name)]
    parts[header] = parts[header].replace(
        b"</w:p>",
        b'<w:ins w:id="950" w:author="Editor" w:date="2026-01-01T00:00:00Z"><w:r><w:t>added</w:t></w:r></w:ins>'
        b"</w:p>",
        1,
    )
    with zipfile.ZipFile(target, "w", zipfile.ZIP_DEFLATED) as z:
        for name, data in parts.items():
            z.writestr(name, data)
    return target


@pytest.mark.parametrize(("accept", "text"), [(True, "headeradded"), (False, "header")])
def test_a_revision_id_in_a_header_is_resolved_and_saved(saved, tmp_path, accept, text):
    doc = oxdocx.Document.open(with_header_insertion(saved, tmp_path / "header.docx"))
    assert doc.section(0).header.paragraphs[0].text == "headeradded"

    assert (doc.accept_revision_id if accept else doc.reject_revision_id)(950) == 1

    assert doc.section(0).header.paragraphs[0].text == text
    again = oxdocx.Document.from_bytes(doc.to_bytes())
    assert again.section(0).header.paragraphs[0].text == text
    assert "w:ins" in tracked(again, 0).xml
