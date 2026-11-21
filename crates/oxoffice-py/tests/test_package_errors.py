"""Each package raises its own exception classes, though the two bindings share the code that looks them up."""

import oxoffice.docx as oxdocx
import oxoffice.pptx as oxpptx
import pytest


def test_a_stale_handle_raises_its_own_package_error():
    doc = oxdocx.Document()
    note = doc.add_footnote(doc.add_paragraph("x"), "note")
    note.delete()
    with pytest.raises(oxdocx.StaleElementError) as word:
        note.paragraphs
    assert not isinstance(word.value, oxpptx.StaleElementError)

    prs = oxpptx.Presentation()
    slide = prs.slides.add_slide(prs.slide_layouts[6])
    box = slide.shapes.add_textbox(0, 0, 100, 100)
    slide.shapes.remove(box)
    with pytest.raises(oxpptx.StaleElementError) as powerpoint:
        box.text
    assert not isinstance(powerpoint.value, oxdocx.StaleElementError)
