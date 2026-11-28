"""Excel workbooks: the Python API of oxoffice-xlsx.

Open a workbook saved by Excel, Google Sheets or LibreOffice, read its cells
as Excel displays them and save it without loss::

    from oxoffice.xlsx import Workbook

    wb = Workbook("review.xlsx")
    ws = wb.sheet("Orders")
    total = ws.cell("F38")
    print(total.formula, total.value, total.text)   # SUM(F2:F37) 206459.11 206,459.11
    rows = ws.values("A1:G5")                       # one native call, nested lists
    for comment in ws.comments:
        print(comment.ref, comment.author, comment.text)
    wb.save("copy.xlsx")

Make a workbook, add a sheet, set the widths of columns, the heights of rows
and the frozen panes::

    wb = Workbook()
    data = wb.add_sheet("Data")
    data.set_column_width("A:C", 20)
    data.set_row_height("1", 30)
    data.freeze_panes("A2")
    wb.save("new.xlsx")

Cells are snapshots read by address. Positions are 0-based, like every index
of the oxoffice packages; A1 references are the spreadsheet's own, from 1.
"""


class XlsxError(Exception):
    """Base class for errors raised by oxoffice.xlsx."""


class PackageError(XlsxError):
    """A file is not a workbook this package opens, or cannot be written."""


class XmlError(XlsxError, ValueError):
    """A part of the workbook is no well-formed SpreadsheetML.

    Also a ``ValueError``: a value holding a character XML 1.0 cannot carry is
    refused with this error.
    """


class CellError(str):
    """An error value of a cell (``#N/A``, ``#DIV/0!``...): a ``str`` holding its code.

    ``cell.value == "#N/A"`` works, and ``isinstance(cell.value, CellError)`` tells an error from a text.
    """

    __slots__ = ()

    def __repr__(self) -> str:
        return f"CellError({str.__repr__(self)})"


from ._native import (  # noqa: E402
    Alignment,
    Border,
    BorderSide,
    Cell,
    Color,
    Comment,
    Fill,
    Font,
    GradientStop,
    HeaderFooter,
    PageBreak,
    PageMargins,
    PageSetup,
    Protection,
    Reply,
    Style,
    Workbook,
    Worksheet,
)

__all__ = [
    "Alignment",
    "Border",
    "BorderSide",
    "Cell",
    "CellError",
    "Color",
    "Comment",
    "Fill",
    "Font",
    "GradientStop",
    "HeaderFooter",
    "PackageError",
    "PageBreak",
    "PageMargins",
    "PageSetup",
    "Protection",
    "Reply",
    "Style",
    "Workbook",
    "Worksheet",
    "XlsxError",
    "XmlError",
]
