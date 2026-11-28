"""The oxoffice.xlsx edit binding layer: each Python type a cell takes, a formula told from a text by
``Formula``, the keywords of clear, set_style, import_csv and the saves, and each refusal's error class.
The semantics are tested in Rust."""

import datetime
import io
import zipfile
from pathlib import Path

import pytest

import oxoffice.xlsx as oxlsx

CORPUS = Path(__file__).resolve().parents[3] / "tests" / "corpus" / "files"


def test_each_python_type_is_written_and_reads_back():
    wb = oxlsx.Workbook()
    ws = wb.sheets[0]
    for cell, value, read, text in [
        ("A1", 3, 3.0, "3"),
        ("A2", 2.5, 2.5, "2.5"),
        ("A3", True, True, "TRUE"),
        ("A4", "007", "007", "007"),
        ("A5", "=A1", "=A1", "=A1"),
        ("A6", oxlsx.CellError("#N/A"), "#N/A", "#N/A"),
        ("A7", datetime.date(2026, 1, 5), 46027.0, "2026-01-05"),
        ("A8", datetime.datetime(2026, 1, 5, 18, 0), 46027.75, "2026-01-05 18:00:00"),
        ("A9", datetime.time(6, 0), 0.25, "06:00:00"),
        ("A10", datetime.timedelta(hours=26), 26 / 24, "26:00:00"),
    ]:
        ws.set_value(cell, value)
        got = ws.cell(cell)
        assert got.value == pytest.approx(read) if isinstance(read, float) else got.value == read, cell
        assert got.text == text, cell
    assert isinstance(ws.cell("A6").value, oxlsx.CellError)
    assert ws.cell("A5").formula is None
    assert ws.cell("A8").datetime == datetime.datetime(2026, 1, 5, 18, 0)
    assert ws.cell("A10").duration == datetime.timedelta(hours=26)
    assert ws.cell("A7").duration is None
    ws.set_value((0, 0), None)
    assert ws.cell("A1").value is None
    with pytest.raises(TypeError, match="a cell takes a number"):
        ws.set_value("B1", [1])
    with pytest.raises(ValueError, match="finite number"):
        ws.set_value("B1", float("nan"))


def test_a_formula_is_told_from_a_text_by_formula():
    wb = oxlsx.Workbook()
    ws = wb.sheets[0]
    ws.write("A1", [[1, 2, oxlsx.Formula("SUM(A1:B1)")], ["=A1", None, oxlsx.Formula("SORT(A1:B1)", spill=True)]])
    assert ws.cell("C1").formula == "SUM(A1:B1)"
    assert ws.cell("A2").value == "=A1" and ws.cell("A2").formula is None
    assert ws.cell("C2").formula_kind == "array"
    ws.set_formula((3, 0), "_xlfn.XLOOKUP(1,A1:A2,B1:B2)", stored=True)
    assert ws.cell("A4").formula == "XLOOKUP(1,A1:A2,B1:B2)"
    ws.set_value("A5", oxlsx.Formula("=A1*2"))
    assert ws.cell("A5").formula == "A1*2"
    assert repr(oxlsx.Formula("A1", spill=True)) == "Formula('A1', spill=True)"
    with pytest.raises(ValueError, match="write `,` between arguments"):
        ws.set_formula("A6", "IF(A1>0;1;0)")
    with pytest.raises(TypeError):
        ws.write("A1", "abc")
    assert wb.recalculation_needed


def test_clear_style_and_number_format_take_their_keywords():
    wb = oxlsx.Workbook()
    ws = wb.sheets[0]
    ws.write("A1", [[1, 2]])
    ws.set_style("A1:B1", bold=True, fill="FFFF00", font_size=12, border="thin", horizontal="center")
    ws.set_number_format("A1", "0.00")
    style = ws.cell("A1").style
    assert style.font.bold and style.font.size == 12 and style.fill.foreground.rgb == "FFFF00"
    assert ws.cell("A1").text == "1.00"
    with pytest.raises(TypeError, match="font_size is a number of points"):
        ws.set_style("A1", font_size=True)
    with pytest.raises(ValueError, match="six hex digits"):
        ws.set_style("A1", fill="yellow")
    ws.clear("A1", formats=True)
    assert ws.cell("A1").value is None and not ws.cell("A1").style.font.bold
    ws.clear("B1", contents=False, formats=True)
    assert ws.cell("B1").value == 2.0 and not ws.cell("B1").style.font.bold


def test_import_csv_takes_a_str_bytes_or_a_path(tmp_path):
    wb = oxlsx.Workbook()
    ws = wb.sheets[0]
    ws.import_csv("north,12\r\nsouth,007\r\n", at="B2")
    assert ws.values("B2:C3") == [["north", 12.0], ["south", "007"]]
    ws.import_csv("﻿x,1".encode(), infer=False)
    assert ws.values("A1:B1") == [["x", "1"]]
    path = tmp_path / "data.csv"
    path.write_bytes(b"TRUE,2026-01-05\n")
    ws.import_csv(path, at="A10")
    assert ws.cell("A10").value is True and ws.cell("B10").text == "2026-01-05"
    with pytest.raises(ValueError, match="not UTF-8"):
        ws.import_csv(b"caf\xe9")
    with pytest.raises(TypeError):
        ws.import_csv(3)


def test_a_refused_edit_raises_xlsx_error_and_drop_cached_values_reaches_every_save(tmp_path):
    wb = oxlsx.Workbook(CORPUS / "excel" / "budget.xlsx")
    ledger = wb.sheet("Ledger")
    before = wb.to_bytes()
    with pytest.raises(oxlsx.XlsxError, match="header of column"):
        ledger.set_value("F4", "Total")
    with pytest.raises(oxlsx.XlsxError, match="merged range"):
        wb.sheet("Budget").set_value("B1", 1)
    assert wb.to_bytes() == before

    def sheet_xml(data):
        with zipfile.ZipFile(io.BytesIO(data)) as package:
            return package.read("xl/worksheets/sheet1.xml").decode()

    dropped = sheet_xml(wb.to_bytes(drop_cached_values=True))
    assert "SUM(B4:E4)</f></c>" in dropped
    assert "<v>37922.35" not in dropped
    for save in (
        lambda path: wb.save(path, drop_cached_values=True),
        lambda path: wb.save_encrypted(path, "pw", drop_cached_values=True),
    ):
        path = tmp_path / "saved.xlsx"
        save(path)
        assert path.exists()
        path.unlink()
    again = oxlsx.Workbook.from_encrypted_bytes(wb.to_encrypted_bytes("pw", drop_cached_values=True), "pw")
    assert again.sheet("Budget").cell("F4").value is None
