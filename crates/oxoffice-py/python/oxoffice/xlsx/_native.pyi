"""Type stubs of the native oxoffice.xlsx bindings."""

import os
from typing import Literal

_Path = str | os.PathLike[str]

class Workbook:
    """An Excel-compatible workbook (``.xlsx``, ``.xlsm``, ``.xltx``, ``.xltm``).

    A file that is not one (a legacy ``.xls``, a binary ``.xlsb``, an
    OpenDocument spreadsheet, a Word or PowerPoint file, a password-protected
    workbook) raises ``PackageError`` with a plain message naming the way out.
    """

    def __init__(self, path: _Path) -> None:
        """Opens the workbook at ``path``."""

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
