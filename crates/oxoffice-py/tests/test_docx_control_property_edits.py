"""docx: a property of a content control of the body (its tag, alias, lock, appearance, placeholder flag, type or data
binding) lives in the control's own properties, which no handle names, so setting it keeps every handle: a handle taken
before the edit reads and edits the same element after it, inside the control too. The edit is made in place on the
body block that holds the control, without saving the document: a refused edit, and an edit that changes nothing,
leave the document as it was, byte for byte. A control of another story (a header) takes the staged route."""
import zipfile

import docx
import pytest
from docx.oxml import parse_xml
from docx.oxml.ns import nsdecls

import oxoffice.docx as oxdocx
from builders import edit_cost_docx
from test_docx_comment_edits import before, handles_taken, reads

PAYLOAD = oxdocx.ContentControlTypePayload


def sdt(tag, content):
    return parse_xml(f'<w:sdt {nsdecls("w")}><w:sdtPr><w:tag w:val="{tag}"/></w:sdtPr><w:sdtContent>{content}'
                     '</w:sdtContent></w:sdt>')


@pytest.fixture(scope="module")
def saved(tmp_path_factory):
    """The document of test_docx_comment_edits, whose body holds a block control (probe0, before the paragraph "spot
    0"), a control inside the eighth paragraph (inline), one inside a cell of the first table (cell) and one inside a
    text box of the last paragraph (boxed), and whose first header holds a control (head)."""
    path = edit_cost_docx(tmp_path_factory.mktemp("controls") / "plain.docx", 12, probes=1)
    d = docx.Document(path)
    d.paragraphs[7]._p.append(sdt("inline", "<w:r><w:t>inline text</w:t></w:r>"))
    d.tables[0].cell(1, 1)._tc.insert(1, sdt("cell", "<w:p><w:r><w:t>cell control</w:t></w:r></w:p>"))
    d.sections[0].header._element.append(sdt("head", "<w:p><w:r><w:t>header control</w:t></w:r></w:p>"))
    box = d.add_paragraph()._p
    box.append(parse_xml(
        f'<w:r {nsdecls("w")}><w:pict xmlns:v="urn:schemas-microsoft-com:vml"><v:shape id="box" '
        'style="width:120pt;height:40pt"><v:textbox><w:txbxContent>'
        '<w:sdt><w:sdtPr><w:tag w:val="boxed"/></w:sdtPr><w:sdtContent><w:p><w:r><w:t>boxed control</w:t></w:r>'
        '</w:p></w:sdtContent></w:sdt></w:txbxContent></v:textbox></v:shape></w:pict></w:r>'))
    d.save(path)
    doc = oxdocx.Document.open(path)
    doc.add_comment_on_text(doc.paragraphs[5].runs[0].text, author="Reviewer", text="a remark")
    out = path.with_name("saved.docx")
    out.write_bytes(doc.to_bytes())
    return out


def located(doc, tag):
    (item,) = [it for it in doc.story_items
               if it.kind == "content_control" and f'<w:tag w:val="{tag}"/>'.encode() in it.xml]
    return oxdocx.ContentLocation(item.story, item.kind, item.index_path)


def the_control(doc, text):
    (control,) = [control for control in doc.content_controls() if control.text == text]
    return control


def taken_with_controls(doc):
    """The handles of test_docx_comment_edits, a handle on each control, and the paragraphs and runs inside them."""
    taken = handles_taken(doc)
    inline = doc.paragraphs[7]
    block = next(p for p in doc.paragraphs if p.text == "control 0")
    more = {
        "inline control": (the_control(doc, "inline text"), lambda h: (h.tag, h.text)),
        "cell control": (the_control(doc, "cell control"), lambda h: (h.tag, h.text)),
        "text box control": (the_control(doc, "boxed control"), lambda h: (h.tag, h.text)),
        "paragraph holding a control": (inline, lambda h: h.text),
        "run beside a control": (inline.runs[1], lambda h: h.text),
        "paragraph inside a control": (block, lambda h: h.text),
        "run inside a control": (block.runs[0], lambda h: h.text),
    }
    taken.update({name: (handle, read, read(handle)) for name, (handle, read) in more.items()})
    return taken


def store(doc):
    return doc.add_custom_xml_store(b"<data><v>bound</v></data>")


# Each setter: the call, and what the control reads after it in a reopened document (from its XML).
SETTERS = {
    "tag": (lambda doc, loc: doc.set_content_control_tag(loc, "renamed"), b'<w:tag w:val="renamed"/>'),
    "alias": (lambda doc, loc: doc.set_content_control_alias(loc, "An alias"), b'<w:alias w:val="An alias"/>'),
    "lock": (lambda doc, loc: doc.set_content_control_lock(loc, "sdtLocked"), b'<w:lock w:val="sdtLocked"/>'),
    "appearance": (lambda doc, loc: doc.set_content_control_appearance(loc, "hidden"), b':appearance w15:val="hidden"'),
    "placeholder": (lambda doc, loc: doc.set_content_control_placeholder(loc, True), b"<w:showingPlcHdr/>"),
    "type": (lambda doc, loc: doc.set_content_control_type(loc, PAYLOAD.date("dd/MM/yyyy")),
             b'<w:dateFormat w:val="dd/MM/yyyy"/>'),
    "binding": (None, b'w:xpath="/data/v"'),  # call() binds to the store opened() adds
}

TAGS = ["probe0", "inline", "cell", "boxed"]
# Every setter edits through one route (the body block that holds the control), which depends on where the control sits
# and not on the property, but for the type and the binding, which the route checks and writes of their own: each
# setter on the block control, the tag on each of the other places, the type and the binding on the cell and the box.
PLACED = ([(setter, "probe0") for setter in sorted(SETTERS)] + [("tag", tag) for tag in TAGS[1:]]
          + [(setter, tag) for setter in ("type", "binding") for tag in ("cell", "boxed")])


class Bound:
    """A document and the id of a store it holds, for the binding setter."""


def opened(saved, setter):
    doc = oxdocx.Document.open(saved)
    holder = Bound()
    holder.doc = doc
    holder.store = store(doc) if setter == "binding" else None
    return holder


def call(holder, setter, loc):
    doc = holder.doc
    if setter == "binding":
        return doc.set_content_control_binding(loc, holder.store, "/data/v")
    return SETTERS[setter][0](doc, loc)


def control_xml(doc, tag):
    return next(it.xml for it in doc.story_items
                if it.kind == "content_control" and f'<w:tag w:val="{tag}"/>'.encode() in it.xml)


@pytest.mark.parametrize(("setter", "tag"), PLACED)
def test_setting_a_body_control_property_keeps_every_handle(saved, setter, tag):
    holder = opened(saved, setter)
    doc = holder.doc
    taken = taken_with_controls(doc)
    call(holder, setter, located(doc, tag))
    # A ContentControl is a snapshot: it reads what it read.
    assert reads(taken) == before(taken)
    # The handles still edit their element.
    # Runs first: setting a cell's text retires the run handles.
    taken["run inside a control"][0].text = "edited inside"
    taken["run beside a control"][0].text = "edited beside"
    taken["paragraph"][0].text = "edited paragraph"
    taken["cell"][0].text = "edited cell"
    again = oxdocx.Document.from_bytes(doc.to_bytes())
    marker = SETTERS[setter][1]
    after_tag = "renamed" if setter == "tag" else tag
    assert marker in control_xml(again, after_tag)
    assert again.paragraphs[3].text == "edited paragraph"
    assert again.tables[0].cell(0, 1).text == "edited cell"
    assert any(p.text == "edited inside" for p in again.paragraphs)
    assert again.paragraphs[7].runs[1].text == "edited beside"


@pytest.mark.parametrize(("setter", "tag"), PLACED)
def test_setting_the_same_property_twice_reads_the_same_document(saved, setter, tag):
    holder = opened(saved, setter)
    doc = holder.doc
    call(holder, setter, located(doc, tag))
    once = doc.to_bytes()
    taken = taken_with_controls(doc)
    after_tag = "renamed" if setter == "tag" else tag
    call(holder, setter, located(doc, after_tag))
    assert doc.to_bytes() == once
    assert reads(taken) == before(taken)


REFUSED = {
    "appearance": lambda holder, loc: holder.doc.set_content_control_appearance(loc, "invisible"),
    "lock": lambda holder, loc: holder.doc.set_content_control_lock(loc, "locked"),
    "tag": lambda holder, loc: holder.doc.set_content_control_tag(loc, "bad\x01tag"),
    "type": lambda holder, loc: holder.doc.set_content_control_type(loc, PAYLOAD.repeating_section_item()),
    "binding to a missing store": lambda holder, loc: holder.doc.set_content_control_binding(
        loc, "{00000000-0000-0000-0000-000000000000}", "/data/v"),
    "binding to a missing node": lambda holder, loc: holder.doc.set_content_control_binding(
        loc, holder.store, "/data/missing"),
}


REFUSED_PLACED = ([(r, "probe0") for r in sorted(REFUSED)] + [("tag", t) for t in TAGS[1:]]
                  + [(r, t) for r in ("type", "binding to a missing node") for t in ("cell", "boxed")])


@pytest.mark.parametrize(("refused", "tag"), REFUSED_PLACED)
def test_a_refused_property_leaves_the_document_as_it_was(saved, refused, tag):
    holder = opened(saved, "binding")
    doc = holder.doc
    unchanged = doc.to_bytes()
    taken = taken_with_controls(doc)
    with pytest.raises((oxdocx.DocxError, ValueError)):
        REFUSED[refused](holder, located(doc, tag))
    assert doc.to_bytes() == unchanged
    assert reads(taken) == before(taken)


@pytest.mark.parametrize("setter", sorted(SETTERS))
def test_a_header_control_property_takes_the_staged_route(saved, setter):
    holder = opened(saved, setter)
    doc = holder.doc
    call(holder, setter, located(doc, "head"))
    after_tag = "renamed" if setter == "tag" else "head"
    again = oxdocx.Document.from_bytes(doc.to_bytes())
    assert SETTERS[setter][1] in control_xml(again, after_tag)
    # Handles taken after the edit read and edit their element.
    doc.paragraphs[3].text = "edited paragraph"
    assert oxdocx.Document.from_bytes(doc.to_bytes()).paragraphs[3].text == "edited paragraph"


@pytest.fixture(scope="module")
def namespaced(tmp_path_factory):
    """A paragraph and a block control (probe) in a body whose w:body element declares a namespace of its own: the
    body saves as it was opened, and a typed edit of it cannot be saved."""
    path = tmp_path_factory.mktemp("controls") / "namespaced.docx"
    d = docx.Document()
    d.add_paragraph("first")
    d.element.body.insert(1, sdt("probe", "<w:p><w:r><w:t>inside</w:t></w:r></w:p>"))
    d.save(path)
    with zipfile.ZipFile(path) as z:
        parts = {name: z.read(name) for name in z.namelist()}
    parts["word/document.xml"] = parts["word/document.xml"].replace(
        b"<w:body>", b'<w:body xmlns:x="urn:example:x">', 1)
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
        for name, data in parts.items():
            z.writestr(name, data)
    return path


@pytest.mark.parametrize("setter", sorted(set(SETTERS) - {"binding"}))
def test_a_control_property_of_a_body_that_cannot_save_is_refused_after_another_edit(namespaced, setter):
    """The edit of a paragraph forgets whether the body saves; the setter asks again and refuses as a save would,
    leaving the control as it was."""
    doc = oxdocx.Document.open(namespaced)
    doc.paragraphs[0].text = "edited"
    control = control_xml(doc, "probe")
    for _ in range(2):
        with pytest.raises(oxdocx.DocxError, match="shadowed `x` namespace"):
            SETTERS[setter][0](doc, located(doc, "probe"))
        assert control_xml(doc, "probe") == control
