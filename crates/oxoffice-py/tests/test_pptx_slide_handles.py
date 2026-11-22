"""pptx: a handle taken before slides move, are inserted, removed or duplicated keeps pointing at its own slide's
content: it reads it, and an edit through it lands on that slide and nowhere else."""
import pytest

import oxoffice.pptx as oxpptx
from oxoffice.pptx import Inches

SLIDES = 4


def deck():
    """A deck of SLIDES slides, each with a text box and a 2x2 table naming their slide; returns the deck and, per
    slide, its handles (slide, box, the box's first run, a table cell)."""
    prs = oxpptx.Presentation()
    handles = []
    for i in range(SLIDES):
        slide = prs.slides.add_slide(prs.slide_layouts[6])
        box = slide.shapes.add_textbox(0, 0, Inches(2), Inches(1))
        box.text_frame.text = f"box {i}"
        table = slide.shapes.add_table(2, 2, 0, Inches(2), Inches(3), Inches(1)).table
        table.cell(1, 1).text = f"cell {i}"
        handles.append((slide, box, box.text_frame.paragraphs[0].runs[0], table.cell(1, 1)))
    return prs, handles


def boxes(prs):
    return [slide.shapes[0].text_frame.text if len(slide.shapes) else None for slide in prs.slides]


def cells(prs):
    return [slide.shapes[1].table.cell(1, 1).text if len(slide.shapes) > 1 else None for slide in prs.slides]


def assert_handles_read_their_slide(prs, handles, order):
    """`order[k]` is the original slide now at position k."""
    for i in set(order) - {None}:
        slide, box, run, cell = handles[i]
        assert prs.slides.index(slide) == order.index(i)
        assert (box.text_frame.text, run.text, cell.text) == (f"box {i}", f"box {i}", f"cell {i}")


def test_handles_follow_their_slide_when_slides_move_or_are_inserted(tmp_path):
    prs, handles = deck()
    prs.slides.move(3, 0)
    assert_handles_read_their_slide(prs, handles, [3, 0, 1, 2])
    prs.slides.move(1, 3)
    assert_handles_read_their_slide(prs, handles, [3, 1, 2, 0])
    prs.slides.add_slide_at(prs.slide_layouts[6], 0)
    assert_handles_read_their_slide(prs, handles, [None, 3, 1, 2, 0])
    prs.slides.add_slide_at(prs.slide_layouts[6], 3)
    order = [None, 3, 1, None, 2, 0]
    assert_handles_read_their_slide(prs, handles, order)

    handles[0][2].text = "run 0"
    handles[1][3].text = "cell 1 edited"
    handles[2][1].text_frame.text = "box 2 edited"
    assert boxes(prs) == [None, "box 3", "box 1", None, "box 2 edited", "run 0"]
    assert cells(prs) == [None, "cell 3", "cell 1 edited", None, "cell 2", "cell 0"]

    prs.save(tmp_path / "moved.pptx")
    assert boxes(oxpptx.Presentation(tmp_path / "moved.pptx")) == boxes(prs)


def test_handles_follow_their_slide_when_slides_are_removed_or_duplicated():
    prs, handles = deck()
    prs.slides.remove(handles[1][0])
    assert_handles_read_their_slide(prs, handles, [0, 2, 3])
    _, box, run, cell = handles[1]
    for read in (lambda: box.text_frame.text, lambda: run.text, lambda: cell.text):
        with pytest.raises(oxpptx.StaleElementError):
            read()
    copy = prs.slides.duplicate(handles[0][0])
    assert prs.slides.index(copy) == 1
    assert_handles_read_their_slide(prs, handles, [0, None, 2, 3])
    prs.slides.move(3, 0)
    assert_handles_read_their_slide(prs, handles, [3, 0, None, 2])

    copy.shapes[0].text_frame.text = "copy"
    handles[3][3].text = "cell 3 edited"
    assert boxes(prs) == ["box 3", "box 0", "copy", "box 2"]
    assert cells(prs) == ["cell 3 edited", "cell 0", "cell 0", "cell 2"]
