"""docx: replacing a picture's image, setting the legend of a chart no other element shows and pointing a hyperlink
whose relationship no other element names at another URL edit package parts and relationships alone, on the live
document: every handle stays, the value reads back after a save, a call that changes nothing leaves the bytes as they
were, a refused call too, and the shared cases (an image part another relationship names, a chart two paragraphs show,
a relationship two links name, a link with an anchor) still keep the other element as it was."""
import io
import zipfile

import docx
import pytest
from docx.opc.constants import RELATIONSHIP_TYPE as RT
from docx.oxml import parse_xml
from docx.oxml.ns import nsdecls
from PIL import Image

import oxoffice.docx as oxdocx
from oxoffice.docx import XL_CHART_TYPE, CategoryChartData, Inches
from builders import edit_cost_docx, noise_png
from test_docx_block_handles import outcome

URL = "https://example.com/a"


@pytest.fixture(scope="module")
def saved(tmp_path_factory):
    """The document test_docx_scaling.py times on, with its pictures: a body of tables, pictures and a header."""
    return edit_cost_docx(tmp_path_factory.mktemp("parts") / "plain.docx", 12, picture_side=8)


def handles(doc):
    """A handle of each body kind, each with how to read it and what it reads now."""
    table = doc.tables[0]
    taken = {
        "paragraph": (doc.paragraphs[3], lambda h: h.text),
        "run": (doc.paragraphs[3].runs[2], lambda h: h.text),
        "table": (table, lambda h: h.cell(1, 1).text),
        "cell": (table.cell(0, 1), lambda h: h.text),
        "header": (doc.section(0).header.paragraphs[0], lambda h: h.text),
    }
    return {name: (handle, read, read(handle)) for name, (handle, read) in taken.items()}


def kept(taken):
    """Every handle reads what it read when it was taken."""
    return {name: outcome(lambda: read(handle)) for name, (handle, read, _) in taken.items()} == {
        name: value for name, (_, _, value) in taken.items()}


def jpeg():
    buffer = io.BytesIO()
    Image.new("RGB", (4, 4), (200, 10, 10)).save(buffer, format="JPEG")
    return buffer.getvalue()


def media(data):
    with zipfile.ZipFile(io.BytesIO(data)) as package:
        return sorted(name for name in package.namelist() if "/media/" in name)


def body_story(doc):
    return next(story for story in doc.stories if story.kind == "body")


# ---------------------------------------------------------------- pictures
@pytest.mark.parametrize("call", ["replace_image", "replace_image_for_story"])
def test_replacing_an_image_keeps_every_handle_and_reads_back(saved, call):
    doc = oxdocx.Document(str(saved))
    taken = handles(doc)
    picture = doc.pictures()[-1]
    new = noise_png(8, 4242)
    if call == "replace_image":
        doc.replace_image(picture.relationship_id, new)
    else:
        doc.replace_image_for_story(body_story(doc), picture.relationship_id, new)
    assert kept(taken)
    assert doc.image_data(picture.relationship_id) == new
    again = oxdocx.Document.from_bytes(doc.to_bytes())
    assert again.image_data(picture.relationship_id) == new
    # The same image again changes nothing.
    unchanged = doc.to_bytes()
    getattr(doc, call)(*([body_story(doc)] if call != "replace_image" else []), picture.relationship_id, new)
    assert doc.to_bytes() == unchanged
    assert kept(taken)


def test_an_image_of_another_format_takes_a_part_of_its_own_and_the_old_one_goes(saved):
    doc = oxdocx.Document(str(saved))
    taken = handles(doc)
    picture = doc.pictures()[-1]
    old = picture.media_part_name
    doc.replace_image(picture.relationship_id, jpeg())
    assert kept(taken)
    assert doc.pictures()[-1].media_part_name.endswith(".jpeg")
    data = doc.to_bytes()
    assert old.lstrip("/") not in media(data)
    again = oxdocx.Document.from_bytes(data)
    assert again.image_data(picture.relationship_id) == jpeg()


def test_an_image_part_another_relationship_names_is_copied_on_write(tmp_path):
    """python-docx stores one image once: the body's and the header's relationships name the same part."""
    made = docx.Document()
    image = noise_png(8, 7)
    made.add_paragraph().add_run().add_picture(io.BytesIO(image), width=docx.shared.Inches(1))
    made.sections[0].header.paragraphs[0].add_run().add_picture(io.BytesIO(image), width=docx.shared.Inches(1))
    made.save(tmp_path / "shared.docx")
    doc = oxdocx.Document(str(tmp_path / "shared.docx"))
    body, header = doc.pictures()[0], doc.pictures()[-1]
    assert body.media_part_name == header.media_part_name
    header_paragraph = doc.section(0).header.paragraphs[0]
    doc.replace_image(body.relationship_id, jpeg())
    assert outcome(lambda: header_paragraph.text) == ""
    assert doc.image_data(body.relationship_id) == jpeg()
    again = oxdocx.Document.from_bytes(doc.to_bytes())
    pictures = again.pictures()
    assert pictures[0].media_part_name != pictures[-1].media_part_name == header.media_part_name
    assert again.image_data(body.relationship_id) == jpeg()
    assert media(again.to_bytes()) == sorted([header.media_part_name.lstrip("/"),
                                              pictures[0].media_part_name.lstrip("/")])


def test_a_header_story_image_is_replaced_through_its_story(saved, tmp_path):
    made = docx.Document()
    made.add_paragraph("body")
    made.sections[0].header.paragraphs[0].add_run().add_picture(io.BytesIO(noise_png(8, 3)),
                                                                 width=docx.shared.Inches(1))
    made.save(tmp_path / "header.docx")
    doc = oxdocx.Document(str(tmp_path / "header.docx"))
    header = next(story for story in doc.stories if story.kind == "header")
    picture = doc.pictures()[-1]
    paragraph = doc.paragraphs[0]
    doc.replace_image_for_story(header, picture.relationship_id, jpeg())
    assert paragraph.text == "body"
    again = oxdocx.Document.from_bytes(doc.to_bytes())
    assert again.pictures()[-1].media_part_name.endswith(".jpeg")


@pytest.mark.parametrize("refused", [
    lambda doc, rid: doc.replace_image(rid, b"not an image"),
    lambda doc, rid: doc.replace_image("rId999", noise_png(8, 1)),
    lambda doc, rid: doc.replace_image_for_story(body_story(doc), "rId999", noise_png(8, 1)),
], ids=["not an image", "no relationship", "no story relationship"])
def test_a_refused_image_replacement_leaves_the_document_and_every_handle(saved, refused):
    doc = oxdocx.Document(str(saved))
    taken = handles(doc)
    unchanged = doc.to_bytes()
    with pytest.raises(oxdocx.DocxError):
        refused(doc, doc.pictures()[-1].relationship_id)
    assert doc.to_bytes() == unchanged
    assert kept(taken)


# ---------------------------------------------------------------- charts
def chart_data():
    data = CategoryChartData()
    data.categories = ["a", "b"]
    data.add_series("s", (1, 2))
    return data


def test_the_legend_of_a_chart_of_its_own_keeps_every_handle_and_reads_back(saved):
    doc = oxdocx.Document(str(saved))
    chart = doc.add_chart(XL_CHART_TYPE.COLUMN_CLUSTERED, chart_data(), Inches(4), Inches(3))
    taken = handles(doc)
    doc.set_chart_legend(chart, visible=True, position="b")
    assert kept(taken)
    assert doc.chart_legend(chart) == {"visible": True, "position": "b", "overlay": False}
    again = oxdocx.Document.from_bytes(doc.to_bytes())
    assert again.chart_legend(again.paragraphs[-1]) == {"visible": True, "position": "b", "overlay": False}
    unchanged = doc.to_bytes()
    doc.set_chart_legend(chart, visible=True, position="b")
    assert doc.to_bytes() == unchanged
    assert kept(taken)


def test_a_refused_legend_leaves_the_document_and_every_handle(saved):
    doc = oxdocx.Document(str(saved))
    taken = handles(doc)
    unchanged = doc.to_bytes()
    with pytest.raises(oxdocx.DocxError):
        doc.set_chart_legend(doc.paragraphs[3], visible=True)
    assert doc.to_bytes() == unchanged
    assert kept(taken)


def test_the_legend_of_a_chart_two_paragraphs_show_is_set_on_a_copy():
    doc = oxdocx.Document()
    chart = doc.add_chart(XL_CHART_TYPE.COLUMN_CLUSTERED, chart_data(), Inches(4), Inches(3))
    doc.add_paragraph("after")
    doc.clone_content(chart, 1)
    copy = doc.paragraphs[1]
    doc.set_chart_legend(copy, visible=True, position="t")
    assert doc.chart_legend(copy)["position"] == "t"
    assert doc.chart_legend(doc.paragraphs[0])["visible"] is False
    again = oxdocx.Document.from_bytes(doc.to_bytes())
    assert again.chart_legend(again.paragraphs[1])["position"] == "t"
    assert again.chart_legend(again.paragraphs[0])["visible"] is False


# ---------------------------------------------------------------- hyperlinks
def link(doc, text):
    return next(h for h in doc.hyperlinks if h.text == text)


def test_pointing_a_link_elsewhere_keeps_every_handle_and_reads_back(saved):
    doc = oxdocx.Document(str(saved))
    doc.paragraphs[5].add_hyperlink("the link", URL)
    taken = handles(doc)
    hyperlink = link(doc, "the link")
    doc.set_hyperlink_url(hyperlink, URL + "b")
    assert kept(taken)
    assert link(doc, "the link").url == URL + "b"
    again = oxdocx.Document.from_bytes(doc.to_bytes())
    assert link(again, "the link").url == URL + "b"
    # The handle taken before still names the link, and the same URL changes nothing.
    unchanged = doc.to_bytes()
    doc.set_hyperlink_url(hyperlink, URL + "b")
    assert doc.to_bytes() == unchanged
    doc.set_hyperlink_url(hyperlink, URL + "c")
    assert link(doc, "the link").url == URL + "c"
    assert kept(taken)


def test_a_refused_url_leaves_the_document_and_every_handle(saved):
    doc = oxdocx.Document(str(saved))
    doc.paragraphs[5].add_hyperlink("the link", URL)
    taken = handles(doc)
    unchanged = doc.to_bytes()
    with pytest.raises(oxdocx.DocxError):
        doc.set_hyperlink_url(link(doc, "the link"), URL + "\x01")
    assert doc.to_bytes() == unchanged
    assert link(doc, "the link").url == URL
    assert kept(taken)


def two_links_one_relationship(path):
    made = docx.Document()
    paragraph = made.add_paragraph("see ")
    rid = paragraph.part.relate_to(URL, RT.HYPERLINK, is_external=True)
    for text in ("first", "second"):
        paragraph._p.append(parse_xml(
            f'<w:hyperlink {nsdecls("w", "r")} r:id="{rid}"><w:r><w:t>{text}</w:t></w:r></w:hyperlink>'))
    made.save(path)
    return path


def test_a_link_whose_relationship_another_link_names_gets_its_own(tmp_path):
    doc = oxdocx.Document(str(two_links_one_relationship(tmp_path / "shared.docx")))
    assert link(doc, "first").relationship_id == link(doc, "second").relationship_id
    doc.set_hyperlink_url(link(doc, "first"), URL + "b")
    assert link(doc, "first").url == URL + "b"
    assert link(doc, "second").url == URL
    again = oxdocx.Document.from_bytes(doc.to_bytes())
    assert link(again, "first").url == URL + "b"
    assert link(again, "second").url == URL
    assert link(again, "first").relationship_id != link(again, "second").relationship_id


def test_a_link_with_an_anchor_becomes_external():
    doc = oxdocx.Document()
    doc.add_paragraph("target").add_run(" here")
    doc.add_bookmark("target", oxdocx.RunRange(start=oxdocx.RunPosition(body_index=0, run_index=0),
                                               end=oxdocx.RunPosition(body_index=0, run_index=1)))
    doc.add_paragraph("see ").add_internal_hyperlink("inside", "target")
    doc.set_hyperlink_url(link(doc, "inside"), URL)
    again = oxdocx.Document.from_bytes(doc.to_bytes())
    assert link(again, "inside").url == URL
    assert link(again, "inside").anchor is None


def test_a_link_with_a_relationship_and_an_anchor_loses_the_anchor(tmp_path):
    made = docx.Document()
    paragraph = made.add_paragraph("see ")
    rid = paragraph.part.relate_to(URL, RT.HYPERLINK, is_external=True)
    paragraph._p.append(parse_xml(
        f'<w:hyperlink {nsdecls("w", "r")} r:id="{rid}" w:anchor="part"><w:r><w:t>both</w:t></w:r></w:hyperlink>'))
    made.save(tmp_path / "both.docx")
    doc = oxdocx.Document(str(tmp_path / "both.docx"))
    assert link(doc, "both").anchor == "part"
    doc.set_hyperlink_url(link(doc, "both"), URL + "b")
    again = oxdocx.Document.from_bytes(doc.to_bytes())
    assert link(again, "both").url == URL + "b"
    assert link(again, "both").anchor is None
