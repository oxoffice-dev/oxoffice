"""A custom XML store, a style linked to a list level and a list restart are edited in place: every handle stays bound."""

import io
import zipfile

import docx
import pytest

import oxoffice.docx as oxdocx
from builders import edit_cost_docx
from test_docx_block_handles import outcome
from test_docx_font_table_edits import handles_taken


@pytest.fixture(scope="module")
def saved(tmp_path_factory):
    return edit_cost_docx(tmp_path_factory.mktemp("parts") / "saved.docx", 12)


def listed(doc):
    """Three numbered items after the saved body; the second one is returned."""
    items = [doc.add_numbered_list_item(f"item {k}") for k in range(3)]
    return items[1]


def linked(doc):
    style = doc.add_style("Probe list", "paragraph").style_id
    definition = doc.add_numbering_definition([oxdocx.ListLevel(format="decimal", text="%1.")])
    return style, doc.add_numbering_instance(definition)


EDITS = {
    # name: (setup(doc) -> made, taken before the handles; edit(doc, made))
    "add_custom_xml_store": (lambda doc: None,
                             lambda doc, _: doc.add_custom_xml_store(b"<data><v>x</v></data>", ["urn:probe"])),
    "link_style_to_numbering": (linked, lambda doc, made: doc.link_style_to_numbering(*made, 0)),
    "restart_numbering": (listed, lambda doc, item: doc.restart_numbering(item, start=5)),
}


@pytest.mark.parametrize("name", sorted(EDITS))
def test_a_part_edit_keeps_every_handle(saved, name):
    doc = oxdocx.Document.open(saved)
    setup, edit = EDITS[name]
    made = setup(doc)
    taken = handles_taken(doc)
    edit(doc, made)
    assert {key: outcome(lambda: read(handle)) for key, (handle, read, _) in taken.items()} == {
        key: before for key, (_, _, before) in taken.items()}
    taken["paragraph"][0].text = "edited paragraph"
    assert doc.paragraphs[3].text == "edited paragraph"


def test_a_restarted_list_keeps_its_item_handles_and_reads_back_after_a_save(saved):
    doc = oxdocx.Document.open(saved)
    items = [doc.add_numbered_list_item(f"item {k}") for k in range(3)]
    old = items[0].numbering[0]
    new = doc.restart_numbering(items[1], start=5)
    assert [item.numbering for item in items] == [(old, 0), (new, 0), (new, 0)]
    items[2].text = "third"
    again = oxdocx.Document.from_bytes(doc.to_bytes())
    tail = again.paragraphs[-3:]
    assert [p.text for p in tail] == ["item 0", "item 1", "third"]
    assert [p.numbering for p in tail] == [(old, 0), (new, 0), (new, 0)]


def test_a_store_and_a_link_read_back_after_a_save(saved):
    doc = oxdocx.Document.open(saved)
    first = doc.add_custom_xml_store(b"<data><v>1</v></data>")
    second = doc.add_custom_xml_store(b"<data><v>2</v></data>", ["urn:probe"])
    assert first != second
    style, instance = linked(doc)
    doc.link_style_to_numbering(style, instance, 0)
    data = doc.to_bytes()
    names = zipfile.ZipFile(io.BytesIO(data)).namelist()
    assert {"customXml/item1.xml", "customXml/item2.xml", "customXml/itemProps1.xml",
            "customXml/itemProps2.xml"} <= set(names)
    # The link is written by the save: the style holds the instance and the level names the style.
    package = zipfile.ZipFile(io.BytesIO(data))
    styles_xml = package.read("word/styles.xml").decode()
    style_xml = styles_xml[styles_xml.index(f'w:styleId="{style}"'):]
    style_xml = style_xml[:style_xml.index("</w:style>")]
    assert f'<w:numId w:val="{instance}"/>' in style_xml
    assert f'<w:pStyle w:val="{style}"/>' in package.read("word/numbering.xml").decode()
    again = oxdocx.Document.from_bytes(data)
    paragraph = again.add_paragraph("styled", style=style)
    assert paragraph.style == style
    # A third store takes the next names and an identity no store holds.
    third = again.add_custom_xml_store(b"<data/>")
    assert third not in (first, second)
    assert "customXml/item3.xml" in zipfile.ZipFile(io.BytesIO(again.to_bytes())).namelist()


REFUSALS = {
    # name: (setup(doc) -> made, edit(doc, made), message)
    "add_custom_xml_store": (lambda doc: None, lambda doc, _: doc.add_custom_xml_store(b"<data>", []), "XML"),
    "add_custom_xml_store empty URI": (lambda doc: None, lambda doc, _: doc.add_custom_xml_store(b"<data/>", [""]),
                                       "schema URI is empty"),
    "link_style_to_numbering": (linked, lambda doc, made: doc.link_style_to_numbering(made[0], 999, 0),
                                "does not exist"),
    # An item whose properties relationship names a missing part: the store's part names and relationship id are
    # reserved before the identity scan refuses.
    "add_custom_xml_store dangling properties": (lambda doc: None,
                                                  lambda doc, _: doc.add_custom_xml_store(b"<data/>"),
                                                  "dangling custom XML edge"),
    "restart_numbering": (lambda doc: None, lambda doc, _: doc.restart_numbering(doc.paragraphs[0]), "not numbered"),
}


@pytest.mark.parametrize("name", sorted(REFUSALS))
def test_a_refused_part_edit_leaves_the_document_as_it_was(saved, name):
    setup, edit, message = REFUSALS[name]
    doc = (oxdocx.Document.from_bytes(with_dangling_store(saved)) if "dangling" in name
           else oxdocx.Document.open(saved))
    made = setup(doc)
    paragraph = doc.paragraphs[3]
    before = doc.to_bytes()
    with pytest.raises(Exception, match=message):
        edit(doc, made)
    assert doc.to_bytes() == before
    assert paragraph.text == doc.paragraphs[3].text


PROPS_REL = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/customXmlProps"


def with_dangling_store(source):
    out = io.BytesIO()
    with zipfile.ZipFile(source) as zin, zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as zout:
        for item in zin.infolist():
            zout.writestr(item, zin.read(item.filename))
        zout.writestr("customXml/item90.xml", b"<data/>")
        zout.writestr("customXml/_rels/item90.xml.rels", (
            '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
            f'<Relationship Id="rId1" Type="{PROPS_REL}" Target="itemProps90.xml"/></Relationships>').encode())
    return out.getvalue()


def test_a_restart_and_a_link_read_only_their_own_list_not_other_content():
    # Another paragraph names an instance no list defines. The restart and the link check the numbering graph and the
    # list they change, not every reference of the content; the new instance does not take the dangling id.
    doc = oxdocx.Document()
    items = [doc.add_numbered_list_item(f"item {k}") for k in range(3)]
    dangling = doc.add_paragraph("dangling")
    dangling.numbering = (77, 0)
    new = doc.restart_numbering(items[1], start=4)
    assert new != 77
    style, instance = linked(doc)
    doc.link_style_to_numbering(style, instance, 0)
    again = oxdocx.Document.from_bytes(doc.to_bytes())
    assert again.paragraphs[-1].numbering == (77, 0)
    assert [p.numbering for p in again.paragraphs[-4:-1]] == [items[0].numbering, (new, 0), (new, 0)]


def missing_level(doc):
    """A list whose last item names a level its definition lacks; the second item is returned."""
    definition = doc.add_numbering_definition([oxdocx.ListLevel(format="decimal", text="%1.")])
    instance = doc.add_numbering_instance(definition)
    items = [doc.add_paragraph(f"item {k}") for k in range(3)]
    for item, level in zip(items, [0, 0, 3]):
        item.numbering = (instance, level)
    return items[1]


def test_a_restart_that_would_carry_an_unresolved_item_is_refused():
    doc = oxdocx.Document()
    item = missing_level(doc)
    before = doc.to_bytes()
    with pytest.raises(Exception, match="level 3"):
        doc.restart_numbering(item, start=4)
    assert doc.to_bytes() == before


def test_a_restart_in_a_body_a_typed_edit_cannot_save_is_refused(tmp_path):
    # A w:body that declares a namespace of its own saves as it was opened; a typed edit of it cannot be saved.
    path = tmp_path / "list.docx"
    source = docx.Document()
    source.add_paragraph("first")
    for k in range(3):
        source.add_paragraph(f"item {k}", style="List Number")
    source.save(path)
    with zipfile.ZipFile(path) as z:
        parts = {name: z.read(name) for name in z.namelist()}
    parts["word/document.xml"] = parts["word/document.xml"].replace(
        b"<w:body>", b'<w:body xmlns:x="urn:example:x">', 1)
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
        for name, data in parts.items():
            z.writestr(name, data)
    doc = oxdocx.Document.open(path)
    before = doc.to_bytes()
    with pytest.raises(Exception):
        doc.restart_numbering(doc.paragraphs[2], start=4)
    assert doc.to_bytes() == before
    # The store and the link do not touch the body and still save.
    doc.add_custom_xml_store(b"<d/>")
    doc.link_style_to_numbering(*linked(doc), 0)
    doc.to_bytes()
