"""A core or application property holding a character XML cannot carry is refused when it is set, and the document
still saves."""

import pytest

import oxoffice.docx as oxdocx


def test_a_property_xml_cannot_carry_is_refused_and_the_document_still_saves():
    doc = oxdocx.Document()
    with pytest.raises(Exception, match="U\\+0007.*XML 1.0 cannot carry"):
        doc.core_properties.title = "Q3\x07"
    oxdocx.Document.from_bytes(doc.to_bytes())


def test_an_application_property_xml_cannot_carry_is_refused():
    doc = oxdocx.Document()
    with pytest.raises(Exception, match="company.*U\\+0007"):
        doc.app_properties.company = "Corp\x07"
    oxdocx.Document.from_bytes(doc.to_bytes())
