"""The oxoffice.xlsx binding layer: a workbook opens from a path or bytes, its sheets are handles that read through
the workbook, a save refuses the source unless asked, and each refusal raises the package's own error class."""

import shutil
from pathlib import Path

import pytest

import oxoffice.docx as docx
import oxoffice.pptx as pptx
import oxoffice.xlsx as oxlsx

CORPUS = Path(__file__).resolve().parents[3] / "tests" / "corpus" / "files"


@pytest.fixture
def multi(tmp_path):
    path = tmp_path / "multi.xlsx"
    shutil.copy(CORPUS / "excel" / "multi.xlsx", path)
    return path


def test_a_workbook_opens_from_a_path_or_bytes_and_lists_its_sheets(multi):
    for wb in (oxlsx.Workbook(multi), oxlsx.Workbook.open(str(multi)), oxlsx.Workbook.from_bytes(multi.read_bytes())):
        sheets = wb.sheets
        assert [(s.name, s.index, s.state, s.kind) for s in sheets] == [
            ("Summary", 0, "visible", "worksheet"),
            ("Sales", 1, "visible", "worksheet"),
            ("Q1 Forecast", 2, "visible", "worksheet"),
            ("Rates", 3, "hidden", "worksheet"),
            ("Values", 4, "visible", "worksheet"),
        ]
        assert sheets[-1].name == "Values"
        assert wb.sheet("q1 forecast").index == 2
        assert wb.date1904 is False
    with pytest.raises(KeyError):
        wb.sheet("Missing")
    assert oxlsx.Workbook(CORPUS / "excel" / "dates1904.xlsx").date1904 is True


def test_a_save_refuses_the_source_unless_asked(multi, tmp_path):
    wb = oxlsx.Workbook(multi)
    before = multi.read_bytes()
    with pytest.raises(oxlsx.PackageError, match="overwrite_source"):
        wb.save(multi)
    with pytest.raises(TypeError):
        wb.save(multi, True)
    assert multi.read_bytes() == before
    wb.save(multi, overwrite_source=True)
    assert multi.read_bytes() == wb.to_bytes()
    locked = tmp_path / "locked.xlsx"
    wb.save_encrypted(locked, "secret")
    with pytest.raises(oxlsx.PackageError, match="password-protected"):
        oxlsx.Workbook(locked)
    assert oxlsx.Workbook.open_encrypted(locked, "secret").to_bytes() == wb.to_bytes()
    again = oxlsx.Workbook.from_encrypted_bytes(wb.to_encrypted_bytes("other"), "other")
    assert [s.name for s in again.sheets] == [s.name for s in wb.sheets]


def test_each_refusal_raises_the_package_error_class(tmp_path):
    import zipfile

    with pytest.raises(oxlsx.PackageError, match="open it with oxoffice docx"):
        oxlsx.Workbook(CORPUS / "word" / "report.docx")
    with pytest.raises(oxlsx.PackageError):
        oxlsx.Workbook.from_bytes(b"not a workbook")
    broken = tmp_path / "broken.xlsx"
    with zipfile.ZipFile(CORPUS / "excel" / "review.xlsx") as source, zipfile.ZipFile(broken, "w") as target:
        for item in source.infolist():
            data = source.read(item)
            target.writestr(item, b"<workbook" if item.filename == "xl/workbook.xml" else data)
    with pytest.raises(oxlsx.XmlError) as error:
        oxlsx.Workbook(broken)
    assert isinstance(error.value, ValueError) and isinstance(error.value, oxlsx.XlsxError)


def sheet_xml(data: bytes, part: str) -> str:
    import io
    import zipfile

    with zipfile.ZipFile(io.BytesIO(data)) as package:
        return package.read(part).decode()


def test_a_new_workbook_is_the_same_bytes_every_time_and_keeps_the_sheets_it_is_given(tmp_path):
    wb = oxlsx.Workbook()
    assert [s.name for s in wb.sheets] == ["Sheet1"]
    assert oxlsx.Workbook().to_bytes() == wb.to_bytes() == oxlsx.Workbook(sheets=["Sheet1"]).to_bytes()
    named = oxlsx.Workbook(sheets=["Data", "Summary"])
    assert [(s.name, s.index) for s in named.sheets] == [("Data", 0), ("Summary", 1)]
    named.save(tmp_path / "named.xlsx")
    assert [s.name for s in oxlsx.Workbook(tmp_path / "named.xlsx").sheets] == ["Data", "Summary"]
    with pytest.raises(TypeError, match="not both"):
        oxlsx.Workbook(tmp_path / "named.xlsx", sheets=["A"])
    for names, error in [([], ValueError), (["a", "A"], oxlsx.XlsxError), (["a:b"], ValueError)]:
        with pytest.raises(error):
            oxlsx.Workbook(sheets=names)


def test_a_sheet_added_is_a_handle_that_stays_valid_and_edits_its_own_sheet():
    wb = oxlsx.Workbook()
    data = wb.add_sheet("Data")
    notes = wb.add_sheet("Notes")
    assert [(s.name, s.index, s.kind) for s in (data, notes)] == [("Data", 1, "worksheet"), ("Notes", 2, "worksheet")]
    data.set_column_width("A:C", 20)
    data.set_row_height("2", 30.5)
    data.freeze_panes("B2")
    notes.freeze_panes((1, 0))
    saved = wb.to_bytes()
    assert '<col min="1" max="3" width="20.7109375" customWidth="1"/>' in sheet_xml(saved, "xl/worksheets/sheet2.xml")
    assert '<row r="2" ht="30.5" customHeight="1"/>' in sheet_xml(saved, "xl/worksheets/sheet2.xml")
    assert 'topLeftCell="B2"' in sheet_xml(saved, "xl/worksheets/sheet2.xml")
    assert 'ySplit="1" topLeftCell="A2"' in sheet_xml(saved, "xl/worksheets/sheet3.xml")
    assert "<cols>" not in sheet_xml(saved, "xl/worksheets/sheet1.xml")
    # The panes of a tuple and of a name are the same panes, and removing them takes them away.
    other = oxlsx.Workbook()
    other.add_sheet("Data").freeze_panes("B2")
    wb2 = oxlsx.Workbook()
    wb2.add_sheet("Data").freeze_panes((1, 1))
    assert other.to_bytes() == wb2.to_bytes()
    data.freeze_panes(None)
    data.freeze_panes()
    assert "<pane" not in sheet_xml(wb.to_bytes(), "xl/worksheets/sheet2.xml")
    assert [s.name for s in oxlsx.Workbook.from_bytes(wb.to_bytes()).sheets] == ["Sheet1", "Data", "Notes"]


@pytest.mark.parametrize(
    ("call", "error", "message"),
    [
        (lambda wb: wb.add_sheet("a/b"), ValueError, "not a sheet name"),
        (lambda wb: wb.add_sheet("SHEET1"), oxlsx.XlsxError, "exists already"),
        (lambda wb: wb.sheets[0].set_column_width("A:", 5), ValueError, "not a column"),
        (lambda wb: wb.sheets[0].set_column_width("A", 0), ValueError, "would hide"),
        (lambda wb: wb.sheets[0].set_column_width("A", float("nan")), ValueError, "from 0 to 255"),
        (lambda wb: wb.sheets[0].set_column_width("A", True), TypeError, "number of characters"),
        (lambda wb: wb.sheets[0].set_column_width("A", "5"), TypeError, "number of characters"),
        (lambda wb: wb.sheets[0].set_column_width("A", docx.Pt(12)), TypeError, "not a Length"),
        (lambda wb: wb.sheets[0].set_row_height("1", pptx.Pt(12)), TypeError, "not a Length"),
        (lambda wb: wb.sheets[0].set_row_height("0", 12), ValueError, "not a row"),
        (lambda wb: wb.sheets[0].set_row_height("1:1048576", 12), oxlsx.XlsxError, "at most 100000"),
        (lambda wb: wb.sheets[0].freeze_panes("A1"), ValueError, "no rows above it"),
        (lambda wb: wb.sheets[0].freeze_panes((1048576, 0)), IndexError, "outside the sheet"),
        (lambda wb: wb.sheets[0].freeze_panes(3), TypeError, "a cell is"),
    ],
)
def test_each_argument_error_raises_the_builtin_or_package_class(call, error, message):
    wb = oxlsx.Workbook()
    before = wb.to_bytes()
    with pytest.raises(error, match=message):
        call(wb)
    assert wb.to_bytes() == before
