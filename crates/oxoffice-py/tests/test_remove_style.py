"""Document.remove_style removes an unused linked pair from either side, and refuses while anything outside the pair
names either side; a numbering level that names a removed list style back stops naming it. On the Word copy of
the corpus report."""

from pathlib import Path

import pytest

import oxoffice.docx as oxdocx

REPORT = Path(__file__).resolve().parents[3] / "tests" / "corpus" / "files" / "word" / "report.docx"


def style_ids(doc):
    return {style.style_id for style in doc.styles}


def test_an_unused_linked_pair_goes_from_either_side():
    doc = oxdocx.Document.from_bytes(REPORT.read_bytes())
    assert doc.remove_style("Titre7Car")
    saved = oxdocx.Document.from_bytes(doc.to_bytes())
    assert not {"Titre7", "Titre7Car"} & style_ids(saved)
    assert {"Titre8", "Titre8Car"} <= style_ids(saved)


def test_a_used_pair_stays_and_the_error_names_the_used_side():
    doc = oxdocx.Document.from_bytes(REPORT.read_bytes())
    with pytest.raises(oxdocx.DocxError, match="'Titre1' is referenced by "):
        doc.remove_style("Titre1Car")
    assert {"Titre1", "Titre1Car"} <= style_ids(doc)


def test_a_list_style_goes_and_its_numbering_level_stops_naming_it():
    doc = oxdocx.Document.from_bytes(REPORT.read_bytes())
    assert doc.remove_style("Listepuces3")
    saved = oxdocx.Document.from_bytes(doc.to_bytes())
    assert "Listepuces3" not in style_ids(saved)
