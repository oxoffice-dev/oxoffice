"""ShapeCollection.walk() lists every shape, those inside groups included, depth first in z-order, a group before its
children, as (depth, shape) pairs whose shapes are live handles."""

import io
import shutil
import zipfile
from pathlib import Path

import pytest

import oxoffice.pptx as oxpptx

CORPUS = Path(__file__).resolve().parents[3] / "tests" / "corpus" / "files"
INCH = oxpptx.Inches(1)


def add_box(shapes, name):
    box = shapes.add_textbox(INCH, INCH, INCH, INCH)
    box.name = name
    box.text_frame.text = name
    return box


@pytest.fixture
def deck(tmp_path):
    """A copy of the corpus deck with a slide holding a, outer [b, inner [c], d], e."""
    path = tmp_path / "deck.pptx"
    shutil.copy(CORPUS / "powerpoint" / "deck.pptx", path)
    prs = oxpptx.Presentation(path)
    slide = prs.slides.add_slide(prs.slide_layouts[len(prs.slide_layouts) - 1])
    add_box(slide.shapes, "a")
    outer = slide.shapes.add_group_shape()
    outer.name = "outer"
    add_box(outer.shapes, "b")
    inner = outer.shapes.add_group_shape()
    inner.name = "inner"
    add_box(inner.shapes, "c")
    add_box(outer.shapes, "d")
    add_box(slide.shapes, "e")
    return prs


def names(walked):
    return [(depth, shape.name) for depth, shape in walked if shape.name in "a outer b inner c d e".split()]


def test_walk_visits_a_group_before_its_children_in_z_order(deck):
    slide = deck.slides[-1]
    assert names(slide.shapes.walk()) == [(0, "a"), (0, "outer"), (1, "b"), (1, "inner"), (2, "c"), (1, "d"),
                                          (0, "e")]
    assert len(slide.shapes.walk()) == len(slide.shapes) + 4


def test_walk_of_a_group_starts_at_depth_zero(deck):
    outer = next(shape for shape in deck.slides[-1].shapes if shape.name == "outer")
    assert names(outer.shapes.walk()) == [(0, "b"), (0, "inner"), (1, "c"), (0, "d")]


def test_walked_shapes_are_live_handles(deck, tmp_path):
    c = next(shape for depth, shape in deck.slides[-1].shapes.walk() if shape.name == "c")
    c.text_frame.text = "changed through the walk"
    deck.save(tmp_path / "out.pptx")
    saved = oxpptx.Presentation(tmp_path / "out.pptx")
    texts = [shape.text_frame.text for _, shape in saved.slides[-1].shapes.walk() if shape.has_text_frame]
    assert "changed through the walk" in texts


def test_walk_covers_layouts_and_masters(deck):
    layout = deck.slide_layouts[0]
    assert [shape.name for _, shape in layout.shapes.walk()] == [shape.name for shape in layout.shapes]
    master = layout.slide_master
    assert len(master.shapes.walk()) >= len(master.shapes)


def bare_shape(id_, name):
    return f'<p:sp><p:nvSpPr><p:cNvPr id="{id_}" name="{name}"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr/></p:sp>'


def test_walk_enters_the_fallback_of_alternate_content():
    block = ('<mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006">'
             '<mc:Choice xmlns:p14="http://schemas.microsoft.com/office/powerpoint/2010/main" Requires="p14">'
             f'{bare_shape(900, "choice")}</mc:Choice>'
             f'<mc:Fallback>{bare_shape(901, "fallback a")}{bare_shape(902, "fallback b")}</mc:Fallback>'
             '</mc:AlternateContent>')
    out = io.BytesIO()
    with zipfile.ZipFile(CORPUS / "powerpoint" / "deck.pptx") as source, zipfile.ZipFile(out, "w") as target:
        for item in source.infolist():
            data = source.read(item)
            if item.filename == "ppt/slides/slide1.xml":
                data = data.replace(b"</p:spTree>", block.encode() + b"</p:spTree>")
            target.writestr(item, data)
    prs = oxpptx.Presentation.from_bytes(out.getvalue())
    walked = prs.slides[0].shapes.walk()
    assert [(depth, shape.name) for depth, shape in walked[-2:]] == [(1, "fallback a"), (1, "fallback b")]
    assert "choice" not in [shape.name for _, shape in walked]
    assert len(walked) == len(prs.slides[0].shapes) + 2
