"""Excel workbooks: the Python API of oxoffice-xlsx.

Open a workbook saved by Excel, Google Sheets or LibreOffice, list its sheets
and save it without loss::

    from oxoffice.xlsx import Workbook

    wb = Workbook("budget.xlsx")
    print([sheet.name for sheet in wb.sheets])
    wb.save("copy.xlsx")

Make a workbook, add a sheet, set the widths of columns, the heights of rows
and the frozen panes::

    wb = Workbook()
    data = wb.add_sheet("Data")
    data.set_column_width("A:C", 20)
    data.set_row_height("1", 30)
    data.freeze_panes("A2")
    wb.save("new.xlsx")

Positions are 0-based, like every index of the oxoffice packages.
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


from ._native import Workbook, Worksheet  # noqa: E402

__all__ = ["PackageError", "Workbook", "Worksheet", "XlsxError", "XmlError"]
