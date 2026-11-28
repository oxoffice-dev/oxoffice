"""The oxoffice.xlsx read bindings: cells are read-only snapshots by A1 reference or 0-based position, values convert
to Python types in one native call, and styles, comments and page setups come back as frozen objects; refusals raise
the package's errors or the built-in ones."""

import datetime
import zipfile
from pathlib import Path

import pytest

import oxoffice.xlsx as oxlsx

CORPUS = Path(__file__).resolve().parents[3] / "tests" / "corpus" / "files"


def test_a_cell_is_a_read_only_snapshot_by_reference_or_position():
    ws = oxlsx.Workbook(CORPUS / "excel" / "review.xlsx").sheet("Orders")
    cell = ws.cell("F3")
    assert cell == ws.cell((2, 5))
    assert (cell.ref, cell.row, cell.column) == ("F3", 2, 5)
    assert (cell.formula, cell.stored_formula, cell.formula_kind, cell.stale) == ("D3*E3", "D3*E3", "shared", False)
    assert isinstance(cell.value, float) and cell.text == "12,816.21" and cell.number_format == "#,##0.00"
    assert cell.datetime is None and ws.cell("C2").datetime == datetime.datetime(2026, 9, 1)
    assert ws.cell("D2").value == 2.0 and isinstance(ws.cell("D2").value, float)
    assert ws.cell("Z99").value is None
    with pytest.raises(AttributeError):
        cell.value = 5
    with pytest.raises(ValueError, match="is not a cell"):
        ws.cell("A1:B2")
    with pytest.raises(IndexError):
        ws.cell((-1, 0))
    with pytest.raises(IndexError):
        ws.cell((0, 16384))
    with pytest.raises(TypeError):
        ws.cell(3)
    assert [c.ref for c in ws.cells("38:40")] == ["A38", "D38", "F38", "A40"]
    assert ws.used_range == "A1:G40"


def test_a_position_read_keeps_the_error_of_the_cell(tmp_path):
    broken = tmp_path / "broken.xlsx"
    with zipfile.ZipFile(CORPUS / "excel" / "review.xlsx") as source, zipfile.ZipFile(broken, "w") as target:
        for item in source.infolist():
            data = source.read(item)
            if item.filename == "xl/worksheets/sheet1.xml":
                data = data.replace(b'<c r="A3" t="s"><v>10</v></c>', b'<c r="A3" t="b"><v>yes</v></c>')
            target.writestr(item, data)
    ws = oxlsx.Workbook(broken).sheets[0]
    for cell in ("A3", (2, 0)):
        with pytest.raises(oxlsx.XmlError, match="not a boolean"):
            ws.cell(cell)


def test_values_convert_every_kind_in_one_call():
    ws = oxlsx.Workbook(CORPUS / "excel" / "multi.xlsx").sheet("Values")
    values = [row[0] for row in ws.values("B2:B12")]
    assert values[0] == "#DIV/0!" and isinstance(values[0], oxlsx.CellError)
    assert repr(values[1]) == "CellError('#N/A')"
    assert values[7:10] == [True, False, True]
    assert values[10] == "Plain, bold and italic, then red and large" and not isinstance(values[10], oxlsx.CellError)
    assert ws.values("C1:C2") == [[None], [None]]
    assert ws.values("A2:B2", text=True) == [["Error #DIV/0!", "#DIV/0!"]]
    with pytest.raises(ValueError, match="read a smaller range"):
        ws.values("A1:XFD1048576")


def test_styles_comments_and_page_setup_are_frozen_snapshots(tmp_path):
    budget = oxlsx.Workbook(CORPUS / "excel" / "budget.xlsx")
    ws = budget.sheet("Budget")
    style = ws.cell("A1").style
    assert (style.font.name, style.font.size, style.font.bold) == ("Cambria", 16.0, True)
    assert (style.fill.kind, style.fill.pattern, style.fill.foreground.rgb) == ("pattern", "solid", "0F766E")
    assert (style.alignment.horizontal, style.border.left.style, style.style_name) == ("center", "none", "Normal")
    theme = budget.sheet("Formats").cell("C2").style.font.color
    assert (theme.rgb, theme.source, theme.index, theme.tint) == ("000000", "theme", 1, 0.0)
    assert ws.merged_ranges == ["A1:J1", "A17:J19"]
    setup = ws.page_setup
    assert (setup.orientation, setup.paper_size, setup.print_area) == ("landscape", 9, "Budget!$A$1:$J$36")
    assert setup.margins.left == 0.5 and setup.header_footer.odd_footer == "&LPrepared by Finance&CPage &P of &N"

    comments = oxlsx.Workbook(CORPUS / "excel" / "threads.xlsx").sheet("Orders").comments
    thread = comments[0]
    assert (thread.ref, thread.kind, thread.author, thread.resolved) == ("C3", "thread", "Corpus Author", False)
    assert [reply.text for reply in thread.replies][1] == "Done, see the updated list."
    assert (comments[1].kind, comments[1].author, comments[1].date) == ("note", "Reviewer", None)

    dates = oxlsx.Workbook(CORPUS / "excel" / "dates1904.xlsx")
    assert dates.sheets[0].to_csv(raw=True, delimiter=";").startswith("Description;Value;Plus 30 days\r\n")
    assert dates.to_markdown().startswith("## Dates\n\n|   | A | B | C |\n")
    assert oxlsx.Workbook(CORPUS / "authored" / "review.xlsx").recalculation_needed is True

    chart = tmp_path / "chart.xlsx"
    with zipfile.ZipFile(CORPUS / "excel" / "review.xlsx") as source, zipfile.ZipFile(chart, "w") as target:
        for item in source.infolist():
            data = source.read(item)
            if item.filename == "xl/_rels/workbook.xml.rels":
                data = data.replace(b"relationships/worksheet", b"relationships/chartsheet")
            target.writestr(item, data)
    charts = oxlsx.Workbook(chart)
    sheet = charts.sheets[0]
    assert (sheet.used_range, sheet.merged_ranges, sheet.comments) == (None, [], [])
    assert charts.to_markdown() == "## Orders (chartsheet)\n"
    for read in (lambda: sheet.cell("A1"), lambda: sheet.values("A1:B2"), sheet.to_csv):
        with pytest.raises(oxlsx.XlsxError, match="is a chartsheet: it holds no cells"):
            read()
    assert sheet.page_setup.paper_size == 1
