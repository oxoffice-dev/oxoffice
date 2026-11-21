"""A settings string holding a character XML cannot carry is refused when it is set, and the document still saves."""

import pytest

import oxoffice.docx as oxdocx


def test_a_protection_hash_holding_a_character_xml_cannot_carry_is_refused_and_the_document_still_saves():
    doc = oxdocx.Document()
    protection = oxdocx.DocumentProtection(mode="readOnly", enforcement=True, hash="ab\x07c", salt="salt")
    with pytest.raises(Exception, match="U\\+0007.*XML 1.0 cannot carry"):
        doc.set_document_protection(protection)
    assert oxdocx.Document.from_bytes(doc.to_bytes()).document_protection is None
