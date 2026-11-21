"""ShapeCollection.walk() lists every shape, those inside groups included, depth first in z-order, a group before its
children, as (depth, shape) pairs whose shapes are live handles."""

import io
import zipfile
from pathlib import Path

import oxoffice.pptx as oxpptx

CORPUS = Path(__file__).resolve().parents[3] / "tests" / "corpus" / "files"
INCH = oxpptx.Inches(1)
NESTED = [(0, "a"), (0, "outer"), (1, "b"), (1, "inner"), (2, "c"), (1, "d"), (0, "e")]


def add_box(shapes, name):
    box = shapes.add_textbox(INCH, INCH, INCH, INCH)
    box.name = name
    box.text_frame.text = name


def add_nested(shapes):
    """Appends a, outer [b, inner [c], d], e to `shapes`."""
    add_box(shapes, "a")
    outer = shapes.add_group_shape()
    outer.name = "outer"
    add_box(outer.shapes, "b")
    inner = outer.shapes.add_group_shape()
    inner.name = "inner"
    add_box(inner.shapes, "c")
    add_box(outer.shapes, "d")
    add_box(shapes, "e")


def nested_deck():
    """The corpus deck with a new last slide holding a, outer [b, inner [c], d], e.

    Called from the test bodies, which conftest runs on a thread with a large stack, never from a fixture, which runs
    on the main thread."""
    prs = oxpptx.Presentation(CORPUS / "powerpoint" / "deck.pptx")
    slide = prs.slides.add_slide(prs.slide_layouts[len(prs.slide_layouts) - 1])
    add_nested(slide.shapes)
    return prs


def walked(shapes):
    return [(depth, shape.name) for depth, shape in shapes.walk()]


def test_walk_visits_a_group_before_its_children_in_z_order():
    shapes = nested_deck().slides[-1].shapes
    placeholders = [(0, shape.name) for shape in shapes[:len(shapes) - 3]]
    assert walked(shapes) == placeholders + NESTED


def test_walk_of_a_group_starts_at_depth_zero():
    outer = next(shape for shape in nested_deck().slides[-1].shapes if shape.name == "outer")
    assert walked(outer.shapes) == [(0, "b"), (0, "inner"), (1, "c"), (0, "d")]


def test_a_walked_handle_edits_its_own_shape(tmp_path):
    prs = nested_deck()
    c = next(shape for _, shape in prs.slides[-1].shapes.walk() if shape.name == "c")
    c.text_frame.text = "changed through the walk"
    prs.save(tmp_path / "out.pptx")
    shapes = oxpptx.Presentation(tmp_path / "out.pptx").slides[-1].shapes
    texts = {shape.name: shape.text_frame.text for _, shape in shapes.walk() if shape.has_text_frame}
    assert {name: texts[name] for name in "abcde"} == {"a": "a", "b": "b", "c": "changed through the walk",
                                                        "d": "d", "e": "e"}


def test_walk_covers_layouts_and_masters():
    prs = oxpptx.Presentation(CORPUS / "powerpoint" / "deck.pptx")
    layout = prs.slide_layouts[0]
    for shapes in (layout.shapes, layout.slide_master.shapes):
        before = [(0, shape.name) for shape in shapes]
        add_nested(shapes)
        assert walked(shapes) == before + NESTED


def bare_shape(id_, name):
    return f'<p:sp><p:nvSpPr><p:cNvPr id="{id_}" name="{name}"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr/></p:sp>'


def test_walk_lists_alternate_content_without_entering_it():
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
    shapes = oxpptx.Presentation.from_bytes(out.getvalue()).slides[0].shapes
    pairs = shapes.walk()
    assert [depth for depth, _ in pairs] == [0] * len(shapes)
    assert [shape.name for shape in shapes[-1].shapes] == ["fallback a", "fallback b"]
    for _, shape in pairs[:-1]:
        shape.name = f"{shape.name} renamed"
    assert all(shape.name.endswith(" renamed") for shape in shapes[:-1])
