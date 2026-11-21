"""docx: the heavy and long underline styles Word writes are read, set and kept, with Word's WdUnderline values."""
import pytest
import oxoffice.docx as oxdocx
from oxoffice.docx import WD_UNDERLINE

HEAVY_AND_LONG = [
    (WD_UNDERLINE.DOTTED_HEAVY, "dottedHeavy"),
    (WD_UNDERLINE.DASH_HEAVY, "dashedHeavy"),
    (WD_UNDERLINE.DOT_DASH_HEAVY, "dashDotHeavy"),
    (WD_UNDERLINE.DOT_DOT_DASH_HEAVY, "dashDotDotHeavy"),
    (WD_UNDERLINE.WAVY_HEAVY, "wavyHeavy"),
    (WD_UNDERLINE.DASH_LONG, "dashLong"),
    (WD_UNDERLINE.WAVY_DOUBLE, "wavyDouble"),
    (WD_UNDERLINE.DASH_LONG_HEAVY, "dashLongHeavy"),
]


@pytest.mark.parametrize(("style", "value"), HEAVY_AND_LONG)
def test_a_heavy_or_long_underline_is_set_read_and_kept(style, value):
    doc = oxdocx.Document()
    run = doc.add_paragraph().add_run("styled")
    run.underline = style
    reopened = oxdocx.Document.from_bytes(doc.to_bytes())
    assert reopened.paragraphs[0].runs[0].underline == style
    assert f'w:u w:val="{value}"' in reopened.part_xml("word/document.xml")
