"""remove_style runs in place: it finds the content that names a style by a walk of what the body writes, raw
fragments included, and keeps every handle."""

import io
import zipfile

import docx
import pytest

import oxoffice.docx as oxdocx
from builders import edit_cost_docx
from test_docx_block_handles import outcome
from test_docx_font_table_edits import handles_taken

W = "http://schemas.openxmlformats.org/wordprocessingml/2006/main"


@pytest.fixture(scope="module")
def saved(tmp_path_factory):
    return edit_cost_docx(tmp_path_factory.mktemp("styles") / "saved.docx", 12)


def test_a_style_removal_keeps_every_handle(saved):
    doc = oxdocx.Document.open(saved)
    style = doc.add_style("Probe unused", "paragraph").style_id
    taken = handles_taken(doc)
    assert doc.remove_style(style) is True
    assert {key: outcome(lambda: read(handle)) for key, (handle, read, _) in taken.items()} == {
        key: before for key, (_, _, before) in taken.items()}
    taken["paragraph"][0].text = "edited paragraph"
    assert doc.paragraphs[3].text == "edited paragraph"
    again = oxdocx.Document.from_bytes(doc.to_bytes())
    assert style not in [s.style_id for s in again.styles]
    assert doc.remove_style(style) is False


def test_a_linked_pair_and_the_numbering_level_naming_it_go_together():
    doc = oxdocx.Document()
    paragraph = doc.add_style("Probe pair", "paragraph").style_id
    definition = doc.add_numbering_definition([oxdocx.ListLevel(format="decimal", text="%1.")])
    instance = doc.add_numbering_instance(definition)
    doc.link_style_to_numbering(paragraph, instance, 0)
    assert doc.remove_style(paragraph) is True
    data = doc.to_bytes()
    package = zipfile.ZipFile(io.BytesIO(data))
    assert f'w:val="{paragraph}"' not in package.read("word/numbering.xml").decode()
    assert f'w:styleId="{paragraph}"' not in package.read("word/styles.xml").decode()


def with_body(body_xml, styles=("Probe",)):
    """A document whose body is `body_xml`, with a character and a paragraph style per name in `styles`."""
    source = docx.Document()
    for name in styles:
        source.styles.add_style(name, docx.enum.style.WD_STYLE_TYPE.CHARACTER)
        source.styles.add_style(name + "Para", docx.enum.style.WD_STYLE_TYPE.PARAGRAPH)
    out = io.BytesIO()
    source.save(out)
    with zipfile.ZipFile(io.BytesIO(out.getvalue())) as z:
        parts = {name: z.read(name) for name in z.namelist()}
    xml = parts["word/document.xml"].decode()
    start = xml.index("<w:body>") + len("<w:body>")
    end = xml.index("<w:sectPr")
    xml = xml[:start] + body_xml + xml[end:]
    parts["word/document.xml"] = xml.encode()
    out = io.BytesIO()
    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
        for name, data in parts.items():
            z.writestr(name, data)
    return out.getvalue()


NAMED = {
    "a tracked insertion": '<w:p><w:ins w:id="1" w:author="a" w:date="2024-01-01T00:00:00Z"><w:r><w:rPr>'
                           '<w:rStyle w:val="Probe"/></w:rPr><w:t>x</w:t></w:r></w:ins></w:p>',
    "an old run style": '<w:p><w:r><w:rPr><w:b/><w:rPrChange w:id="2" w:author="a"><w:rPr><w:rStyle w:val="Probe"/>'
                        '</w:rPr></w:rPrChange></w:rPr><w:t>x</w:t></w:r></w:p>',
    "the paragraph mark": '<w:p><w:pPr><w:rPr><w:rStyle w:val="Probe"/></w:rPr></w:pPr><w:r><w:t>x</w:t></w:r></w:p>',
    "a text box": '<w:p><w:r><mc:AlternateContent><mc:Choice Requires="wps"><w:drawing/></mc:Choice>'
                  '<mc:Fallback><w:pict><v:shape><v:textbox><w:txbxContent><w:p><w:r><w:rPr>'
                  '<w:rStyle w:val="Probe"/></w:rPr><w:t>boxed</w:t></w:r></w:p></w:txbxContent></v:textbox>'
                  '</v:shape></w:pict></mc:Fallback></mc:AlternateContent></w:r></w:p>',
    "a raw block with another prefix": '<x:customXml xmlns:x="' + W + '" x:element="probe"><x:p><x:r><x:rPr>'
                                       '<x:rStyle x:val="Probe"/></x:rPr><x:t>x</x:t></x:r></x:p></x:customXml>',
    "a content control": '<w:sdt><w:sdtPr><w:rPr><w:rStyle w:val="Probe"/></w:rPr></w:sdtPr><w:sdtContent>'
                         '<w:p><w:r><w:t>x</w:t></w:r></w:p></w:sdtContent></w:sdt>',
    "a table cell": '<w:tbl><w:tblGrid><w:gridCol w:w="100"/></w:tblGrid><w:tr><w:tc><w:p><w:r><w:rPr>'
                    '<w:rStyle w:val="Probe"/></w:rPr><w:t>x</w:t></w:r></w:p></w:tc></w:tr></w:tbl>',
}


@pytest.mark.parametrize("where", sorted(NAMED))
def test_a_style_named_by_content_is_refused_and_the_document_is_unchanged(where):
    doc = oxdocx.Document.from_bytes(with_body(NAMED[where]))
    style = next(s.style_id for s in doc.styles if s.name == "Probe")
    before = doc.to_bytes()
    with pytest.raises(Exception, match="referenced by document content"):
        doc.remove_style(style)
    assert doc.to_bytes() == before
    # Another style no content names still goes.
    other = next(s.style_id for s in doc.styles if s.name == "ProbePara")
    assert doc.remove_style(other) is True
