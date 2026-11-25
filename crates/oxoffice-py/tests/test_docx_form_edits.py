"""docx: fill_form and reset_form read the controls they name, and write each changed control in the body block that
holds it, in place, in one edit: a call that names controls of several blocks writes those blocks alone. A value
written inside a control keeps every handle, the paragraphs and runs inside the control too, which then read the
value. A refused call, and a call that changes nothing, leave the document as it was, byte for byte. A control of
another story (a header) and a legacy form field take the staged route."""
import io
import zipfile

import pytest

import oxoffice.docx as oxdocx
from test_docx_comment_edits import before, handles_taken, reads
from test_docx_control_property_edits import control_xml, saved, store, taken_with_controls  # noqa: F401

TAGS = ["probe0", "inline", "cell", "boxed"]


def target(tag):
    return oxdocx.FormFieldTarget.content_control_tag(tag)


def rule(tag):
    return oxdocx.FormFieldRule(target=target(tag), reset=oxdocx.FormResetValue.placeholder("Type here"))


def assignment(tag, text):
    return oxdocx.FormFieldAssignment(target=target(tag), value=oxdocx.FormFieldValue.text(text))


def fill(doc, *tags, text="filled"):
    return doc.fill_form([rule(tag) for tag in tags], [assignment(tag, text) for tag in tags])


# The handles inside the control a call writes, and those around it, read its value after it.
INSIDE = {"probe0": {"paragraph inside a control", "run inside a control"}, "inline": {"paragraph holding a control"},
          "cell": {"table"}, "boxed": set()}


def kept(taken, tags):
    """What the handles taken read before a call that writes the controls `tags`, outside those controls."""
    skipped = set().union(*(INSIDE[tag] for tag in tags))
    return {name: value for name, value in before(taken).items() if name not in skipped}


def reads_outside(taken, tags):
    skipped = set().union(*(INSIDE[tag] for tag in tags))
    return {name: value for name, value in reads(taken).items() if name not in skipped}


@pytest.mark.parametrize("tag", TAGS)
def test_filling_a_body_control_keeps_every_handle(saved, tag):
    doc = oxdocx.Document.open(saved)
    taken = taken_with_controls(doc)
    assert fill(doc, tag) == (1,)
    # A ContentControl is a snapshot: it reads what it read.
    assert reads_outside(taken, [tag]) == kept(taken, [tag])
    if tag == "probe0":
        assert taken["paragraph inside a control"][0].text == "filled"
        assert taken["run inside a control"][0].text == "filled"
    # The handles still edit their element.
    taken["run beside a control"][0].text = "edited beside"
    taken["paragraph"][0].text = "edited paragraph"
    taken["cell"][0].text = "edited cell"
    again = oxdocx.Document.from_bytes(doc.to_bytes())
    assert b">filled<" in control_xml(again, tag)
    assert again.paragraphs[3].text == "edited paragraph"
    assert again.tables[0].cell(0, 1).text == "edited cell"
    assert again.paragraphs[7].runs[1].text == "edited beside"


def test_one_call_writes_the_controls_of_several_blocks(saved):
    doc = oxdocx.Document.open(saved)
    taken = taken_with_controls(doc)
    tags = ["probe0", "cell", "boxed"]
    assert fill(doc, *tags) == (1, 1, 1)
    assert reads_outside(taken, tags) == kept(taken, tags)
    again = oxdocx.Document.from_bytes(doc.to_bytes())
    for tag in tags:
        assert b">filled<" in control_xml(again, tag)
    assert b">inline text<" in control_xml(again, "inline")


@pytest.mark.parametrize("tag", TAGS)
def test_resetting_a_body_control_shows_its_placeholder_and_keeps_every_handle(saved, tag):
    doc = oxdocx.Document.open(saved)
    taken = taken_with_controls(doc)
    assert doc.reset_form([rule(tag)]) == (1,)
    assert reads_outside(taken, [tag]) == kept(taken, [tag])
    taken["paragraph"][0].text = "edited paragraph"
    again = oxdocx.Document.from_bytes(doc.to_bytes())
    assert b"<w:showingPlcHdr/>" in control_xml(again, tag)
    assert b">Type here<" in control_xml(again, tag)
    assert again.paragraphs[3].text == "edited paragraph"


@pytest.mark.parametrize("tag", TAGS)
def test_writing_the_same_value_twice_reads_the_same_document(saved, tag):
    doc = oxdocx.Document.open(saved)
    fill(doc, tag)
    once = doc.to_bytes()
    taken = handles_taken(doc)
    fill(doc, tag)
    assert doc.to_bytes() == once
    assert reads(taken) == before(taken)


REFUSED = {
    "unknown target": lambda doc: fill(doc, "missing"),
    "target named twice": lambda doc: doc.fill_form([rule("probe0"), rule("probe0")], []),
    "assignment without its rule": lambda doc: doc.fill_form([rule("probe0")], [assignment("cell", "x")]),
    "value of another kind": lambda doc: doc.fill_form(
        [rule("cell")], [oxdocx.FormFieldAssignment(target=target("cell"), value=oxdocx.FormFieldValue.checked(True))]),
    # A refusal of the second target leaves the first one as it was.
    "second target refused": lambda doc: doc.fill_form(
        [rule("probe0"), rule("cell")],
        [assignment("probe0", "x"),
         oxdocx.FormFieldAssignment(target=target("cell"), value=oxdocx.FormFieldValue.selected_index(2))]),
}


@pytest.mark.parametrize("refused", sorted(REFUSED))
def test_a_refused_form_call_leaves_the_document_as_it_was(saved, refused):
    doc = oxdocx.Document.open(saved)
    unchanged = doc.to_bytes()
    taken = taken_with_controls(doc)
    with pytest.raises((oxdocx.DocxError, ValueError)):
        REFUSED[refused](doc)
    assert doc.to_bytes() == unchanged
    assert reads(taken) == before(taken)


def stored_items(data):
    with zipfile.ZipFile(io.BytesIO(data)) as z:
        return b"".join(z.read(name) for name in z.namelist() if name.startswith("customXml/item"))


def test_a_bound_control_writes_its_store_in_the_same_call(saved):
    doc = oxdocx.Document.open(saved)
    doc.set_content_control_binding(
        next(oxdocx.ContentLocation(it.story, it.kind, it.index_path) for it in doc.story_items
             if it.kind == "content_control" and b'<w:tag w:val="probe0"/>' in it.xml),
        store(doc), "/data/v")
    taken = taken_with_controls(doc)
    fill(doc, "probe0", text="new value")
    assert reads_outside(taken, ["probe0"]) == kept(taken, ["probe0"])
    data = doc.to_bytes()
    assert b"<v>new value</v>" in stored_items(data)
    assert b">new value<" in control_xml(oxdocx.Document.from_bytes(data), "probe0")


def test_a_header_control_takes_the_staged_route(saved):
    doc = oxdocx.Document.open(saved)
    assert fill(doc, "head") == (1,)
    assert b">filled<" in control_xml(oxdocx.Document.from_bytes(doc.to_bytes()), "head")
    # Handles taken after the call read and edit their element.
    doc.paragraphs[3].text = "edited paragraph"
    assert oxdocx.Document.from_bytes(doc.to_bytes()).paragraphs[3].text == "edited paragraph"


def test_a_footnote_control_takes_the_staged_route(saved):
    doc = oxdocx.Document.open(saved)
    doc.add_footnote(doc.paragraphs[3], "note text")
    (item,) = [it for it in doc.story_items if it.story.kind == "footnote" and it.kind == "paragraph"
               and it.text.endswith("note text")]
    doc.insert_content_control(oxdocx.ContentLocation(item.story, item.kind, item.index_path), "block", "rich_text",
                               tag="noted")
    taken = handles_taken(doc)
    assert fill(doc, "noted") == (1,)
    assert reads(taken) == before(taken)
    assert b">filled<" in control_xml(oxdocx.Document.from_bytes(doc.to_bytes()), "noted")


def test_a_legacy_field_takes_the_staged_route(saved):
    doc = oxdocx.Document.open(saved)
    (item,) = [it for it in doc.story_items if it.story.kind == "body" and it.kind == "paragraph"
               and it.text == doc.paragraphs[3].text]
    doc.insert_legacy_form_field(oxdocx.StoryRunPosition(item=item, run_index=1),
                                 oxdocx.LegacyFormFieldDefinition.text_input(value="old"))
    (field,) = doc.legacy_form_fields()
    legacy = oxdocx.FormFieldTarget.legacy(field.source_part, field.ordinal)
    rules = [oxdocx.FormFieldRule(target=legacy, reset=oxdocx.FormResetValue.value(oxdocx.FormFieldValue.text(""))), rule("probe0")]
    values = [oxdocx.FormFieldAssignment(target=legacy, value=oxdocx.FormFieldValue.text("new")),
              assignment("probe0", "filled")]
    assert doc.fill_form(rules, values) == (1, 1)
    again = oxdocx.Document.from_bytes(doc.to_bytes())
    assert [f.value for f in again.legacy_form_fields()] == ["new"]
    assert b">filled<" in control_xml(again, "probe0")
