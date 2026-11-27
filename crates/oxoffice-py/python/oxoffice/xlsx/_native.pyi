"""Type stubs of the native oxoffice.xlsx bindings."""

import os
from collections.abc import Sequence
from typing import Literal, overload

_Path = str | os.PathLike[str]

class Workbook:
    """An Excel-compatible workbook (``.xlsx``, ``.xlsm``, ``.xltx``, ``.xltm``).

    A file that is not one (a legacy ``.xls``, a binary ``.xlsb``, an
    OpenDocument spreadsheet, a Word or PowerPoint file, a password-protected
    workbook) raises ``PackageError`` with a plain message naming the way out.
    """

    @overload
    def __init__(self) -> None:
        """A new workbook with one empty sheet, ``Sheet1``.

        Calibri 11 under the Office 2013-2022 theme. The same call gives the
        same bytes.
        """

    @overload
    def __init__(self, path: _Path) -> None:
        """Opens the workbook at ``path``."""

    @overload
    def __init__(self, *, sheets: Sequence[str]) -> None:
        """A new workbook with an empty sheet for each name of ``sheets``.

        Raises ``ValueError`` for a name Excel would not accept (see
        ``add_sheet``) or for no name at all, and ``XlsxError`` when a name
        repeats another in any case.
        """

    @staticmethod
    def open(path: _Path) -> Workbook:
        """Opens the workbook at ``path``."""

    @staticmethod
    def from_bytes(data: bytes) -> Workbook:
        """Opens a workbook from the bytes of a file."""

    @staticmethod
    def open_encrypted(path: _Path, password: str) -> Workbook:
        """Opens the password-protected workbook at ``path``."""

    @staticmethod
    def from_encrypted_bytes(data: bytes, password: str) -> Workbook:
        """Opens a password-protected workbook from the bytes of a file."""

    @property
    def sheets(self) -> tuple[Worksheet, ...]:
        """The sheets, in the order of the workbook's tabs."""

    def sheet(self, name: str) -> Worksheet:
        """The sheet named ``name``, the exact name first, then in any case.

        Raises ``KeyError`` when there is none.
        """

    def add_sheet(self, name: str) -> Worksheet:
        """Adds an empty sheet after the others and gives it.

        The name follows Excel's rules: 1 to 31 characters, none of
        ``: \\ / ? * [ ]``, no apostrophe at either end, not ``History``.
        Raises ``ValueError`` for a name that breaks them and ``XlsxError``
        when another sheet has the name in any case.
        """

    @property
    def date1904(self) -> bool:
        """Whether dates count from 1904 rather than 1900."""

    def save(self, path: _Path, *, overwrite_source: bool = False) -> None:
        """Saves to ``path``, staged and renamed so a failed save changes nothing.

        The extension (``.xlsx``, ``.xlsm``, ``.xltx``, ``.xltm``) selects the
        class. A workbook opened from a file refuses to replace it unless
        ``overwrite_source`` is true.
        """

    def to_bytes(self) -> bytes:
        """The bytes of the workbook as it stands."""

    def save_encrypted(self, path: _Path, password: str, *, overwrite_source: bool = False) -> None:
        """Saves to ``path`` protected by ``password``, as ``save`` does."""

    def to_encrypted_bytes(self, password: str) -> bytes:
        """The bytes of the workbook protected by ``password``."""

class Worksheet:
    """A sheet of a workbook, valid through every edit of the workbook."""

    @property
    def name(self) -> str:
        """The name, as its tab shows it."""

    @property
    def index(self) -> int:
        """The 0-based position among the workbook's tabs."""

    @property
    def state(self) -> Literal["visible", "hidden", "veryHidden"]:
        """Whether the sheet shows in the tabs, or is hidden or very hidden."""

    @property
    def kind(self) -> Literal["worksheet", "chartsheet", "dialogsheet", "macrosheet"]:
        """What the sheet holds."""

    def set_column_width(self, columns: str, width: float) -> None:
        """Sets the width of columns, in characters: ``"B"`` or ``"A:C"``.

        The width is what Excel's Column Width box shows for the default
        font, above 0 and at most 255. A ``Length`` or a ``bool`` raises
        ``TypeError``, a width out of range or a span that is not columns
        ``ValueError``, a sheet that holds no cells ``XlsxError``.
        """

    def set_row_height(self, rows: str, height: float) -> None:
        """Sets the height of rows, in points: ``"3"`` or ``"2:5"``.

        Above 0 and at most 409.5, at most 100,000 rows at once; a row the
        sheet does not hold is written. Raises the errors of
        ``set_column_width``.
        """

    def freeze_panes(self, cell: str | tuple[int, int] | None) -> None:
        """Freezes the panes above and left of ``cell``, or removes them.

        ``"B2"`` keeps row 1 and column A in view; a tuple is ``(row,
        column)`` counted from 0. ``None`` removes the frozen panes; the
        argument is required, so a call without one is a ``TypeError``. ``A1``,
        which has nothing above or left of it, raises ``ValueError``.
        """
