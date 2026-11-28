"""Every length argument of oxoffice.docx and oxoffice.pptx reads its value by one rule. None clears where the property
clears. A Length is taken as given, even when it rounds to zero. A bare 0 is accepted. A bare nonzero int is EMU, and
it is refused when it would round to zero in the unit the property is stored in: twips (635 EMU) or half-points (6350
EMU) in Word, hundredths of a point (127 EMU) in PowerPoint. bool and float are refused.

Each row is one argument: how to set it on a new document, how to read back what was stored (in EMU), and what it
does with None."""

import io
import re
import xml.etree.ElementTree as ET
from dataclasses import dataclass
from typing import Callable

import pytest

import oxoffice.docx as oxdocx
import oxoffice.pptx as oxpptx
from builders import noise_png
from oxoffice.docx.shared import Cm, Emu, Pt, Twips
from oxoffice.pptx.util import Inches as PInches, Length as PLength, Pt as PPt

TWIPS, HALF_POINTS, CENTIPOINTS, EMU = 635, 6350, 127, 1
W = "{http://schemas.openxmlformats.org/wordprocessingml/2006/main}"
PNG = noise_png(8, 0)
NATIVE_PICTURE_SIDE = 8 * 12700  # 8 pixels at 72 dpi, the size a picture without one takes
MOVIE = b"\x00\x00\x00\x18ftypmp42" + bytes(64)


@dataclass(frozen=True)
class Arg:
    """One length argument. `target` builds a new document and returns (set, get): set(value) passes the value to the
    argument, get() reads back the stored length in EMU, or None when unset; get is None when nothing reads it back.
    `none` says what None does: "clears", "refused" (TypeError), "omits" (the argument keeps its default) or "zero".
    `zero` is False where the property itself refuses zero. `valid` is a value the property stores. `floats` is True
    where a float means something else (a line multiple). `omitted` is what an omitted argument reads back, where
    that is not the value read before."""

    target: Callable
    unit: int
    none: str
    zero: bool = True
    valid: object = None
    floats: bool = False
    package: str = "docx"
    omitted: object = None


ARGS = {}


def arg(name, target, unit, none, **options):
    spec = Arg(target, unit, none, **options)
    name = f"pptx {name}" if spec.package == "pptx" else name
    assert name not in ARGS, name
    ARGS[name] = spec


def emu(value):
    return None if value is None else int(value)


# --- Word -------------------------------------------------------------------------------------------------------------

def paragraph_format(attribute):
    def target():
        fmt = oxdocx.Document().add_paragraph("x").paragraph_format
        return lambda value: setattr(fmt, attribute, value), lambda: emu(getattr(fmt, attribute))
    return target


for attribute in ("space_before", "space_after", "left_indent", "right_indent", "first_line_indent"):
    arg(f"ParagraphFormat.{attribute}", paragraph_format(attribute), TWIPS, "clears")


def font(attribute):
    def target():
        run = oxdocx.Document().add_paragraph("x").runs[0]
        return lambda value: setattr(run.font, attribute, value), lambda: emu(getattr(run.font, attribute))
    return target


arg("Font.character_spacing", font("character_spacing"), TWIPS, "clears")
arg("Font.size", font("size"), HALF_POINTS, "clears")


def tab_stop():
    tabs = oxdocx.Document().add_paragraph("x").paragraph_format.tab_stops
    return lambda value: tabs.add_tab_stop(value), lambda: emu(tabs[0].position) if len(tabs) else None


arg("TabStops.add_tab_stop(position)", tab_stop, TWIPS, "refused")


def default_font_size():
    doc = oxdocx.Document()
    return lambda value: setattr(doc, "default_font_size", value), lambda: emu(doc.default_font_size)


arg("Document.default_font_size", default_font_size, HALF_POINTS, "clears")


def default_paragraph_format(keyword):
    def target():
        doc = oxdocx.Document()
        return (lambda value: doc.set_default_paragraph_format(**{keyword: value}),
                lambda: emu(getattr(doc.default_paragraph_format, keyword)))
    return target


for keyword in ("space_before", "space_after"):
    arg(f"Document.set_default_paragraph_format({keyword})", default_paragraph_format(keyword), TWIPS, "clears")


def style_xml(doc, style_id, path, attribute, scale):
    """The value of `attribute` on the element at `path` of style `style_id`, times `scale`, or None."""
    root = ET.fromstring(doc.part_xml("word/styles.xml"))
    style = next(item for item in root.iter(f"{W}style") if item.get(f"{W}styleId") == style_id)
    element = style.find(path)
    value = None if element is None else element.get(f"{W}{attribute}")
    return None if value is None else int(value) * scale


STYLE_KEYWORDS = {
    "font_size": ("w:rPr/w:sz", "val", HALF_POINTS),
    "space_before": ("w:pPr/w:spacing", "before", TWIPS),
    "space_after": ("w:pPr/w:spacing", "after", TWIPS),
    "left_indent": ("w:pPr/w:ind", "left", TWIPS),
    "right_indent": ("w:pPr/w:ind", "right", TWIPS),
    "first_line_indent": ("w:pPr/w:ind", "firstLine", TWIPS),
    "tab_stops": ("w:pPr/w:tabs/w:tab", "pos", TWIPS),
}


def style(method, keyword):
    path, attribute, scale = STYLE_KEYWORDS[keyword]
    path = path.replace("w:", W)

    def target():
        doc = oxdocx.Document()
        if method == "add_style":
            def apply(value):
                doc.add_style("Probe", **{keyword: [(value, 0, None)] if keyword == "tab_stops" else value})
        else:
            doc.add_style("Probe")

            def apply(value):
                doc.set_style("Probe", **{keyword: [(value, 0, None)] if keyword == "tab_stops" else value})

        def read():
            return style_xml(doc, "Probe", path, attribute, scale) if "Probe" in [item.style_id for item in
                                                                                  doc.styles] else None
        return apply, read
    return target


for method in ("add_style", "set_style"):
    for keyword, (_, _, unit) in STYLE_KEYWORDS.items():
        arg(f"Document.{method}({keyword})", style(method, keyword), unit,
            "refused" if keyword == "tab_stops" else "omits")


SECTION_LENGTHS = ("page_width", "page_height", "margin_top", "margin_right", "margin_bottom", "margin_left", "gutter",
                   "header_distance", "footer_distance")


def update_section(keyword):
    def target():
        doc = oxdocx.Document()
        read = {"column_spacing": lambda: emu(doc.sections[0].column_spacing)}.get(
            keyword, lambda: emu(getattr(doc.section(0), keyword)))
        return lambda value: doc.update_section(0, **{keyword: value}), read
    return target


for keyword in SECTION_LENGTHS + ("column_spacing",):
    arg(f"Document.update_section({keyword})", update_section(keyword), TWIPS, "omits",
        zero=keyword not in ("page_width", "page_height"), valid=Pt(612) if keyword.startswith("page") else None)


def section_handle(attribute):
    def target():
        section = oxdocx.Document().section(0)
        return lambda value: setattr(section, attribute, value), lambda: emu(getattr(section, attribute))
    return target


for attribute in SECTION_LENGTHS + ("top_margin", "right_margin", "bottom_margin", "left_margin"):
    arg(f"SectionHandle.{attribute}", section_handle(attribute), TWIPS, "refused",
        zero=attribute not in ("page_width", "page_height"), valid=Pt(612) if attribute.startswith("page") else None)


def word_table():
    return oxdocx.Document().add_table(2, 2)


def table_attribute(attribute):
    def target():
        table = word_table()
        return lambda value: setattr(table, attribute, value), lambda: emu(getattr(table, attribute))
    return target


arg("Table.indent", table_attribute("indent"), TWIPS, "clears")
arg("Table.width", table_attribute("width"), TWIPS, "refused")


def margins(owner, method, side):
    index = ("top", "right", "bottom", "left").index(side)

    def target():
        table = word_table()
        handle = table if owner == "table" else table.cell(0, 0)
        reader = "cell_margins" if owner == "table" else "margins"

        def apply(value):
            sides = dict.fromkeys(("top", "right", "bottom", "left"), Pt(1))
            getattr(handle, method)(**{**sides, side: value})

        def read():
            current = getattr(handle, reader)
            return None if current is None else emu(current[index])
        return apply, read
    return target


for side in ("top", "right", "bottom", "left"):
    arg(f"Table.set_cell_margins({side})", margins("table", "set_cell_margins", side), TWIPS, "refused")
    arg(f"Cell.set_margins({side})", margins("cell", "set_margins", side), TWIPS, "refused")


def grid_widths():
    table = word_table()
    other = table.grid_widths[1]
    return lambda value: setattr(table, "grid_widths", [value, other]), lambda: emu(table.grid_widths[0])


def column_width():
    table = word_table()
    return lambda value: table.set_column_width(0, value), lambda: emu(table.grid_widths[0])


def row_height():
    row = word_table().rows[0]
    return lambda value: setattr(row, "height", value), lambda: emu(row.height)


def cell_width():
    cell = word_table().cell(0, 0)
    return lambda value: setattr(cell, "width", value), lambda: emu(cell.width)


arg("Table.grid_widths", grid_widths, TWIPS, "refused", zero=False)
arg("Table.set_column_width(width)", column_width, TWIPS, "refused")
arg("Row.height", row_height, TWIPS, "refused")
arg("Cell.width", cell_width, TWIPS, "refused")


def list_level(keyword):
    attribute = {"left_indent": "left", "hanging_indent": "hanging"}[keyword]

    def target():
        doc = oxdocx.Document()

        def apply(value):
            doc.add_numbering_definition([oxdocx.ListLevel(**{keyword: value})])

        def read():
            if "/word/numbering.xml" not in doc.part_names():
                return None
            numbering = ET.fromstring(doc.part_xml("word/numbering.xml"))
            ind = numbering.find(f"{W}abstractNum/{W}lvl/{W}pPr/{W}ind")
            value = None if ind is None else ind.get(f"{W}{attribute}")
            return None if value is None else int(value) * TWIPS
        return apply, read
    return target


# An omitted indent is the level's default: half an inch, with a quarter-inch hanging indent.
for keyword, default in (("left_indent", 720 * TWIPS), ("hanging_indent", 360 * TWIPS)):
    arg(f"ListLevel({keyword})", list_level(keyword), TWIPS, "omits", omitted=default)


def picture(owner, keyword):
    other = "height" if keyword == "width" else "width"

    def target():
        doc = oxdocx.Document()
        paragraph = doc.add_paragraph("x")
        add = {"Document": lambda: doc.add_picture, "Run": lambda: paragraph.runs[0].add_picture,
               "Cell": lambda: doc.add_table(1, 1).cell(0, 0).add_picture}[owner]()
        made = []

        def apply(value):
            sizes = {keyword: value, other: None if value is None else Pt(12)}
            made.append(add(PNG, "probe.png", **sizes))
        return apply, lambda: emu(getattr(made[-1], keyword)) if made else None
    return target


for owner in ("Document", "Run", "Cell"):
    for keyword in ("width", "height"):
        arg(f"{owner}.add_picture({keyword})", picture(owner, keyword), EMU, "omits", zero=False,
            omitted=NATIVE_PICTURE_SIDE)


def picture_size(method, keyword):
    other = "height" if keyword == "width" else "width"

    def target():
        doc = oxdocx.Document()
        doc.add_picture(PNG, "probe.png")

        def apply(value):
            sizes = {keyword: value, other: Pt(12)}
            if method == "set_picture_size":
                doc.set_picture_size(doc.pictures()[0], sizes["width"], sizes["height"])
            else:
                doc.set_picture_sizes_for_relationship(doc.pictures()[0].relationship_id, sizes["width"],
                                                       sizes["height"])
        return apply, lambda: emu(getattr(doc.pictures()[0], keyword))
    return target


for method in ("set_picture_size", "set_picture_sizes_for_relationship"):
    for keyword in ("width", "height"):
        arg(f"Document.{method}({keyword})", picture_size(method, keyword), EMU, "refused", zero=False)


def chart(keyword):
    def target():
        doc = oxdocx.Document()
        data = oxdocx.CategoryChartData()
        data.categories = ["a", "b"]
        data.add_series("s", (1, 2))
        sizes = {"width": Pt(200), "height": Pt(100)}
        return lambda value: doc.add_chart(oxdocx.XL_CHART_TYPE.COLUMN_CLUSTERED, data,
                                           **{**sizes, keyword: value}), None
    return target


def watermark(owner, kind, keyword):
    def target():
        doc = oxdocx.Document()
        sizes = {"width": Pt(200), "height": Pt(100)}
        if owner == "Document":
            return lambda value: doc.set_image_watermark(PNG, "mark.png", **{**sizes, keyword: value}), None
        section = doc.section(0)
        if kind == "text":
            sizes["font_size"] = Pt(36)
            return lambda value: section.set_text_watermark("DRAFT", **{**sizes, keyword: value}), None
        return lambda value: section.set_image_watermark(PNG, "mark.png", **{**sizes, keyword: value}), None
    return target


for keyword in ("width", "height"):
    arg(f"Document.add_chart({keyword})", chart(keyword), EMU, "refused", zero=False)
    arg(f"Document.set_image_watermark({keyword})", watermark("Document", "image", keyword), EMU, "refused",
        zero=False)
    arg(f"SectionHandle.set_image_watermark({keyword})", watermark("SectionHandle", "image", keyword), EMU,
        "refused", zero=False)
    arg(f"SectionHandle.set_text_watermark({keyword})", watermark("SectionHandle", "text", keyword), EMU,
        "refused", zero=False)
arg("SectionHandle.set_text_watermark(font_size)", watermark("SectionHandle", "text", "font_size"), HALF_POINTS,
    "refused", zero=False)


def picture_options(keyword):
    def target():
        doc = oxdocx.Document()
        made = []

        def apply(value):
            options = {keyword: (value, Pt(1), Pt(1), Pt(1)) if keyword == "distances" else value}
            made.append(doc.add_picture(PNG, "probe.png", options=oxdocx.PictureOptions(placement="square",
                                                                                         **options)))
        return apply, None
    return target


arg("PictureOptions(horizontal_offset)", picture_options("horizontal_offset"), EMU, "omits")
arg("PictureOptions(vertical_offset)", picture_options("vertical_offset"), EMU, "omits")
arg("PictureOptions(distances)", picture_options("distances"), EMU, "refused")


# --- PowerPoint -------------------------------------------------------------------------------------------------------

def slide():
    prs = oxpptx.Presentation()
    return prs, prs.slides.add_slide(prs.slide_layouts[6])


def textbox():
    _, page = slide()
    shape = page.shapes.add_textbox(PInches(1), PInches(1), PInches(2), PInches(1))
    shape.text_frame.text = "x"
    return shape


def shape_attribute(attribute, make=textbox):
    def target():
        shape = make()
        return lambda value: setattr(shape, attribute, value), lambda: emu(getattr(shape, attribute))
    return target


def connector():
    _, page = slide()
    return page.shapes.add_connector(1, PInches(1), PInches(1), PInches(2), PInches(2))


for attribute in ("left", "top", "width", "height"):
    arg(f"Shape.{attribute}", shape_attribute(attribute), EMU, "refused", package="pptx")
for attribute in ("begin_x", "begin_y", "end_x", "end_y"):
    arg(f"Shape.{attribute}", shape_attribute(attribute, connector), EMU, "refused", package="pptx")


def pptx_chart_data():
    data = oxpptx.CategoryChartData()
    data.categories = ["a", "b"]
    data.add_series("s", (1, 2))
    return data


BOX = ("left", "top", "width", "height")
ADDERS = {
    "add_textbox": (BOX, lambda shapes, box: shapes.add_textbox(*box)),
    "add_shape": (BOX, lambda shapes, box: shapes.add_shape(1, *box)),
    "add_connector": (("begin_x", "begin_y", "end_x", "end_y"), lambda shapes, box: shapes.add_connector(1, *box)),
    "add_table": (BOX, lambda shapes, box: shapes.add_table(2, 2, *box)),
    "add_chart": (BOX, lambda shapes, box: shapes.add_chart(oxpptx.XL_CHART_TYPE.COLUMN_CLUSTERED, *box,
                                                            pptx_chart_data())),
    "add_movie": (BOX, lambda shapes, box: shapes.add_movie(io.BytesIO(MOVIE), *box, mime_type="video/mp4")),
    "add_picture": (BOX, lambda shapes, box: shapes.add_picture(io.BytesIO(PNG), *box)),
}


def adder(method, position):
    names, call = ADDERS[method]

    def target():
        _, page = slide()
        made = []

        def apply(value):
            box = [PInches(1), PInches(1), PInches(2), PInches(1)]
            box[position] = value
            if method == "add_picture" and value is None:
                box[2:] = [None, None]
            made.append(call(page.shapes, box))
        return apply, lambda: emu(getattr(made[-1], names[position])) if made else None
    return target


for method, (names, _) in ADDERS.items():
    for position, name in enumerate(names):
        size = position >= 2 and method != "add_connector"
        none = "omits" if method == "add_picture" and size else "refused"
        arg(f"ShapeCollection.{method}({name})", adder(method, position), EMU, none, package="pptx",
            zero=not (size and method in ("add_table", "add_chart", "add_movie")),
            omitted=NATIVE_PICTURE_SIDE if none == "omits" else None)


def slide_size(attribute):
    def target():
        prs = oxpptx.Presentation()
        return lambda value: setattr(prs, attribute, value), lambda: emu(getattr(prs, attribute))
    return target


for attribute in ("slide_width", "slide_height"):
    arg(f"Presentation.{attribute}", slide_size(attribute), EMU, "refused", zero=False, valid=PInches(10),
        package="pptx")


def line_width():
    shape = textbox()
    return lambda value: setattr(shape.line, "width", value), lambda: emu(shape.line.width)


def shadow(attribute):
    def target():
        shape = textbox()
        return lambda value: setattr(shape.shadow, attribute, value), lambda: emu(getattr(shape.shadow, attribute))
    return target


arg("LineFormat.width", line_width, EMU, "zero", package="pptx")
arg("ShadowFormat.blur_radius", shadow("blur_radius"), EMU, "refused", package="pptx")
arg("ShadowFormat.distance", shadow("distance"), EMU, "refused", package="pptx")


def pptx_table():
    _, page = slide()
    return page.shapes.add_table(2, 2, PInches(1), PInches(1), PInches(4), PInches(1)).table


def column_or_row(kind):
    def target():
        table = pptx_table()
        item, attribute = (table.columns[0], "width") if kind == "column" else (table.rows[0], "height")
        return lambda value: setattr(item, attribute, value), lambda: emu(getattr(item, attribute))
    return target


arg("Column.width", column_or_row("column"), EMU, "refused", zero=False, package="pptx")
arg("Row.height", column_or_row("row"), EMU, "refused", zero=False, package="pptx")


def margin(owner, side):
    def target():
        handle = pptx_table().cell(0, 0) if owner == "Cell" else textbox().text_frame
        attribute = f"margin_{side}"
        return lambda value: setattr(handle, attribute, value), lambda: emu(getattr(handle, attribute))
    return target


for side in ("left", "right", "top", "bottom"):
    arg(f"Cell.margin_{side}", margin("Cell", side), EMU, "clears", package="pptx")
    arg(f"TextFrame.margin_{side}", margin("TextFrame", side), EMU, "clears", package="pptx")


def pptx_paragraph(attribute):
    def target():
        paragraph = textbox().text_frame.paragraphs[0]
        return lambda value: setattr(paragraph, attribute, value), lambda: emu(getattr(paragraph, attribute))
    return target


for attribute in ("space_before", "space_after"):
    arg(f"Paragraph.{attribute}", pptx_paragraph(attribute), CENTIPOINTS, "clears", floats=True, package="pptx")
for attribute in ("left_indent", "right_indent", "first_line_indent"):
    arg(f"Paragraph.{attribute}", pptx_paragraph(attribute), EMU, "clears", package="pptx")


def pptx_font(attribute):
    def target():
        run = textbox().text_frame.paragraphs[0].runs[0]
        return lambda value: setattr(run.font, attribute, value), lambda: emu(getattr(run.font, attribute))
    return target


arg("Font.size", pptx_font("size"), CENTIPOINTS, "clears", zero=False, package="pptx")
arg("Font.spacing", pptx_font("spacing"), CENTIPOINTS, "clears", package="pptx")


# --- The rule ---------------------------------------------------------------------------------------------------------

def stored(value, unit):
    """What a property stored in `unit` keeps of `value` EMU: the whole units, truncated toward zero."""
    value = int(value)
    return (abs(value) // unit) * unit * (-1 if value < 0 else 1)


def valid(spec):
    if spec.valid is not None:
        return spec.valid
    return PPt(12) if spec.package == "pptx" else Pt(12)


def zeros(spec):
    return [0, PPt(0), PInches(0)] if spec.package == "pptx" else [0, Pt(0), Twips(0), Cm(0)]


def label(name):
    """The argument as the error messages name it."""
    return re.escape(name.removeprefix("pptx "))


def rows(select=lambda spec: True):
    return [pytest.param(name, id=name) for name, spec in ARGS.items() if select(spec)]


def zero_rows():
    return [pytest.param(name, value, id=f"{name}-{type(value).__name__}")
            for name, spec in ARGS.items() if spec.zero for value in zeros(spec)]


@pytest.mark.parametrize(("name", "value"), zero_rows())
def test_a_bare_zero_and_a_zero_length_are_accepted(name, value):
    apply, read = ARGS[name].target()
    apply(value)
    if read:
        assert read() == 0


@pytest.mark.parametrize("name", rows())
def test_a_length_is_stored_as_given(name):
    spec = ARGS[name]
    apply, read = spec.target()
    apply(valid(spec))
    if read:
        assert read() == stored(valid(spec), spec.unit)


@pytest.mark.parametrize("name", rows(lambda spec: spec.unit > EMU))
def test_a_bare_integer_that_rounds_to_zero_is_refused(name):
    """12, and the largest bare integers that round to zero, one unit less than a unit on each side: a property tagged
    with a smaller unit than the one it is stored in takes them."""
    spec = ARGS[name]
    apply, read = spec.target()
    before = read() if read else None
    unit = {TWIPS: "twips", HALF_POINTS: "half-points", CENTIPOINTS: "hundredths of a point"}[spec.unit]
    for value in (12, spec.unit - 1, -(spec.unit - 1)):
        with pytest.raises(ValueError, match=f"^{label(name)}: bare integer {value} EMU would round to zero {unit};"):
            apply(value)
        if read:
            assert read() == before, (name, value)


@pytest.mark.parametrize("name", rows())
def test_a_bare_integer_is_emu(name):
    """The smallest bare integer a rounding property keeps, one unit (635 is one twip), is stored. Where one unit is
    outside the property's range, and where nothing rounds, a bare integer of the valid length is."""
    spec = ARGS[name]
    apply, read = spec.target()
    value = spec.unit if spec.unit > EMU and spec.zero else int(valid(spec))
    assert type(value) is int
    apply(value)
    if read:
        assert read() == value


@pytest.mark.parametrize("name", rows(lambda spec: spec.unit > EMU and spec.zero))
def test_a_length_that_rounds_to_zero_is_taken_as_given(name):
    spec = ARGS[name]
    apply, read = spec.target()
    apply(PLength(12) if spec.package == "pptx" else Emu(12))
    if read:
        assert read() == 0


@pytest.mark.parametrize("name", rows())
def test_bool_and_float_are_refused(name):
    apply, _ = ARGS[name].target()
    for value in (True, False, 1.5):
        if ARGS[name].floats and value == 1.5:
            continue
        with pytest.raises(TypeError, match=f"^{label(name)} requires a Length"):
            apply(value)


@pytest.mark.parametrize("name", rows())
def test_none(name):
    spec = ARGS[name]
    apply, read = spec.target()
    if spec.none == "refused":
        with pytest.raises(TypeError, match=f"^{label(name)} requires a Length or a bare integer EMU$"):
            apply(None)
        return
    if spec.none in ("clears", "zero"):
        apply(valid(spec))
    before = read() if read else None
    apply(None)
    if spec.none == "clears":
        assert read() is None
    elif spec.none == "zero":
        assert read() == 0
    elif read:
        assert read() == (before if spec.omitted is None else spec.omitted)


class Index:
    """An int-like object that is not an int, as a numpy integer is: it has only `__index__`."""

    def __init__(self, value):
        self.value = value

    def __index__(self):
        return self.value


@pytest.mark.parametrize("name", ["ParagraphFormat.left_indent", "pptx Shape.left"])
def test_an_int_like_object_is_a_bare_integer(name):
    spec = ARGS[name]
    apply, read = spec.target()
    apply(Index(int(Pt(12))))
    assert read() == int(Pt(12))
    if spec.unit > EMU:
        with pytest.raises(ValueError, match=f"^{label(name)}: bare integer 12 EMU would round to zero"):
            apply(Index(12))
    else:
        apply(Index(12))
        assert read() == 12


def test_a_document_is_unchanged_by_a_refused_length():
    doc = oxdocx.Document()
    paragraph = doc.add_paragraph("x")
    paragraph.paragraph_format.space_before = Pt(6)
    before = doc.to_bytes()
    for value in (12, True, 1.5):
        with pytest.raises((TypeError, ValueError)):
            paragraph.paragraph_format.space_before = value
    assert doc.to_bytes() == before
