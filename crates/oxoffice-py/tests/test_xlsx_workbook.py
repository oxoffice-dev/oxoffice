"""The oxoffice.xlsx binding layer: a workbook opens from a path or bytes, its sheets are handles that read through
the workbook, a save refuses the source unless asked, and each refusal raises the package's own error class."""

import shutil
from pathlib import Path

import pytest

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
