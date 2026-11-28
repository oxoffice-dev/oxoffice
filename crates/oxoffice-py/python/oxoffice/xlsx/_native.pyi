"""Type stubs of the native oxoffice.xlsx bindings."""

import datetime
import os
from collections.abc import Sequence
from typing import Literal, overload

from . import CellError

_Path = str | os.PathLike[str]
_Value = float | str | bool | CellError | None
_CellAddress = str | tuple[int, int]
_Input = (
    float
    | int
    | str
    | bool
    | CellError
    | None
    | datetime.datetime
    | datetime.date
    | datetime.time
    | datetime.timedelta
    | Formula
)

class Formula:
    """A formula, for ``Worksheet.write`` and ``set_value``: a str is never a formula.

    ``text`` is the formula as Excel displays it after its ``=`` (an ``=`` is accepted), or as the file stores it
    with ``stored`` (``_xlfn.XLOOKUP(...)``); ``spill`` writes a dynamic array formula that spills from its cell:
    ``True`` (Excel finds the range; Google Sheets shows the first value alone) or its range, ``"J2:J5"`` from the
    cell, which Google Sheets spills over too.
    """

    def __init__(self, text: str, stored: bool = False, spill: bool | str = False) -> None: ...
    @property
    def text(self) -> str: ...
    @property
    def stored(self) -> bool: ...
    @property
    def spill(self) -> bool | str: ...

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

    @property
    def recalculation_needed(self) -> bool:
        """Whether the cached values of formulas may be out of date: the file asks for a recalculation on open,
        or an edit since the workbook was opened changed a workbook that holds a formula."""

    def to_markdown(self) -> str:
        """The workbook as Markdown: ``## <name>`` and a table per sheet, hidden ones included.

        A chartsheet, dialogsheet or macrosheet is ``## <name> (chartsheet)``, with no table.

        Each table is headed by the column letters and each row led by its number, so every value keeps its
        address. The texts are the displayed ones; ``|`` is escaped and line breaks are written ``<br>``.
        """

    def save(self, path: _Path, *, overwrite_source: bool = False, drop_cached_values: bool = False) -> None:
        """Saves to ``path``, staged and renamed so a failed save changes nothing.

        The extension (``.xlsx``, ``.xlsm``, ``.xltx``, ``.xltm``) selects the
        class. A workbook opened from a file refuses to replace it unless
        ``overwrite_source`` is true. ``drop_cached_values`` drops the cached
        value of every formula, so that LibreOffice, which does not calculate
        on open by default, computes them (a function it lacks then shows an
        error); Excel and Google Sheets calculate an edited workbook anyway.
        """

    def to_bytes(self, *, drop_cached_values: bool = False) -> bytes:
        """The bytes of the workbook as it stands."""

    def save_encrypted(
        self, path: _Path, password: str, *, overwrite_source: bool = False, drop_cached_values: bool = False
    ) -> None:
        """Saves to ``path`` protected by ``password``, as ``save`` does."""

    def to_encrypted_bytes(self, password: str, *, drop_cached_values: bool = False) -> bytes:
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

    def set_value(self, cell: _CellAddress, value: _Input) -> None:
        """Writes ``value`` into ``cell``, keeping its style.

        A number, a ``str`` (never read as a formula; ``""`` clears), a ``bool``, a ``CellError``, ``None`` (the
        content cleared), a ``datetime``, ``date``, ``time`` or ``timedelta`` (its serial; a cell shown as General
        takes ``yyyy-mm-dd``, ``yyyy-mm-dd hh:mm:ss``, ``hh:mm:ss`` or ``[h]:mm:ss``), or a ``Formula``. A write into
        a shared formula's range turns the group's other cells into plain formulas. A date in a column of the
        default width shows ``#####`` in Excel until the column is widened (``set_column_width``): Excel widens it
        when a date is typed, not when a file says it. Raises ``XlsxError`` for a
        cell of an array formula written alone, a value under a merged range but its first cell, a table header
        that would no longer name its column, a cell of a pivot table; ``ValueError`` for a value a cell cannot
        hold; ``TypeError`` for another type. A refusal changes nothing.
        """

    def set_formula(
        self, cell: _CellAddress, formula: str, *, stored: bool = False, spill: bool | str = False
    ) -> None:
        """Writes a formula into ``cell``, as Excel displays it after its ``=`` (an ``=`` is accepted).

        ``stored`` takes the form the file stores (``_xlfn.XLOOKUP(...)``); ``spill`` writes a dynamic array
        formula that Excel spills from the cell: ``True``, or its range (``"J2:J5"``, from the cell, its other cells
        empty) so that Google Sheets, which takes the range from the file, spills it too. A formula Excel would repair (``IF(B2>0;1;0)``, ``B2*``,
        ``IF(1,2,3,4)``) raises ``ValueError``. Written without a cached value, the workbook is calculated on open.
        """

    def write(self, cell: _CellAddress, rows: Sequence[Sequence[_Input]]) -> None:
        """Writes ``rows`` from ``cell`` down and right, each value as ``set_value`` writes it.

        A row shorter than another leaves the cells after it as they are.
        """

    def clear(self, range: str, *, contents: bool = True, formats: bool = False) -> None:
        """Clears ``range`` (``"A1:C10"``, ``"B3"``, ``"A:C"``, ``"2:5"``): its contents, its formats, or both.

        The formats of whole columns or rows go from the columns or rows themselves too. A whole array formula is
        removed; part of one raises ``XlsxError``.
        """

    def set_number_format(self, range: str, code: str) -> None:
        """Sets the number format code (``"0.00"``, ``"yyyy-mm-dd"``) of every cell of ``range``."""

    def set_style(
        self,
        range: str,
        *,
        bold: bool | None = None,
        italic: bool | None = None,
        underline: bool | None = None,
        strike: bool | None = None,
        font_name: str | None = None,
        font_size: float | None = None,
        font_color: str | None = None,
        fill: str | None = None,
        border: str | None = None,
        border_color: str | None = None,
        horizontal: str | None = None,
        vertical: str | None = None,
        wrap_text: bool | None = None,
        number_format: str | None = None,
    ) -> None:
        """Changes the parts of the style of every cell of ``range`` that are given; ``None`` leaves one as it is.

        Colours are ``"RRGGBB"``; ``fill`` is a solid fill, ``border`` the style of the four sides (``"thin"``,
        ``"medium"``, ``"thick"``, ``"double"``, ``"none"``...). Identical style records are reused, others
        appended. Whole columns (``"A:C"``) or rows (``"2:5"``) take the style themselves, so a cell typed later takes
        it; a range of cells writes at most 1,000,000 of them.
        """

    def import_csv(self, source: str | bytes | os.PathLike[str], *, at: str = "A1", infer: bool = True) -> None:
        """Writes a CSV from the cell ``at``: ``source`` is its text, its UTF-8 bytes or the path of its file.

        The rectangle it covers is written, an empty field clearing its cell. With ``infer``, a field is a number
        (no leading zero, at most 15 significant digits), ``TRUE`` / ``FALSE``, a date ``YYYY-MM-DD`` with a time
        or not, else a str (a leading ``=`` included); without, every field is a str.
        """

    @property
    def used_range(self) -> str | None:
        """The smallest range holding every cell with a value or a formula (``"A1:F16"``), ``None`` for none.

        Read once. A chartsheet, dialogsheet or macrosheet has none, no merges and no comments; a read of its cells
        (``cell``, ``values``, ``cells``, ``to_csv``) raises ``XlsxError``.
        """

    def cell(self, cell: _CellAddress) -> Cell:
        """The cell at an A1 reference (``"B3"``) or a 0-based ``(row, column)``.

        A cell the sheet does not write reads as empty. A chartsheet, dialogsheet or macrosheet raises
        ``XlsxError``; a reference that names no cell ``ValueError``, a position outside the sheet
        ``IndexError``.
        """

    def values(self, range: str, text: bool = False) -> list[list[_Value]] | list[list[str]]:
        """The values of ``range``, one list per row, in one native call.

        ``range`` is ``"A1:C10"``, ``"B3"``, whole columns ``"A:C"`` or rows ``"2:5"`` (clipped to the used
        range). Numbers are ``float`` (dates too: serial days), errors ``CellError``, an empty cell ``None``.
        With ``text``, the displayed texts, ``""`` for an empty cell.
        """

    def cells(self, range: str) -> list[Cell]:
        """The cells of ``range`` that hold a value or a formula, row by row."""

    @property
    def merged_ranges(self) -> list[str]:
        """The merged ranges, ``["A1:J1", ...]``."""

    @property
    def comments(self) -> list[Comment]:
        """The notes and comment threads, by cell, row by row."""

    @property
    def page_setup(self) -> PageSetup:
        """The page setup: margins, paper, scale, header and footer, breaks, print area and titles."""

    def to_csv(self, *, raw: bool = False, delimiter: str = ",", bom: bool = False) -> str:
        """The sheet as CSV, from A1 to the end of the used range, CRLF line ends, RFC 4180 quoting.

        The displayed texts (what Excel's CSV export writes), or the stored values with ``raw`` (the
        shortest number, dates as serials, ``TRUE`` / ``FALSE``, error codes). ``bom`` starts the text with
        a byte order mark.
        """

class Cell:
    """A cell, read by its address: a snapshot an edit does not change, its attributes read only."""

    @property
    def ref(self) -> str:
        """The A1 reference, ``"B3"``."""

    @property
    def row(self) -> int:
        """The 0-based row."""

    @property
    def column(self) -> int:
        """The 0-based column."""

    @property
    def value(self) -> _Value:
        """``float``, ``str``, ``bool``, ``CellError`` or ``None``; for a formula, the value its producer cached."""

    @property
    def formula(self) -> str | None:
        """The formula as Excel displays it after its ``=`` (``XLOOKUP(...)``, a shared member's moved form)."""

    @property
    def stored_formula(self) -> str | None:
        """The formula as the file stores it (``_xlfn.XLOOKUP(...)``)."""

    @property
    def formula_kind(self) -> Literal["shared", "array", "data_table"] | None:
        """The kind of the cell's formula, or of the array formula or data table whose range holds the cell;
        ``None`` for a plain formula or none."""

    @property
    def array_range(self) -> str | None:
        """The range of the array formula or data table the cell is in."""

    @property
    def stale(self) -> bool:
        """Whether the cached value of the formula, or of the array formula whose range holds the cell, may be out
        of date."""

    @property
    def number_format(self) -> str:
        """The number format code, a built-in id resolved."""

    @property
    def text(self) -> str:
        """The value as Excel displays it in a column wide enough for it, en-US."""

    @property
    def style(self) -> Style:
        """The resolved style."""

    @property
    def datetime(self) -> datetime.datetime | None:
        """The number as a date and time when its format shows a date, a time or both (not a duration)."""

    @property
    def duration(self) -> datetime.timedelta | None:
        """The number as a duration when its format shows one (``[h]:mm``, ``[mm]:ss``); ``None`` when negative."""

class Comment:
    """A note or a comment thread on a cell."""

    @property
    def ref(self) -> str:
        """The cell, ``"C3"``."""

    @property
    def kind(self) -> Literal["note", "thread"]: ...
    @property
    def author(self) -> str | None:
        """Whoever wrote it, ``None`` when the file names nobody."""

    @property
    def date(self) -> str | None:
        """When it was written, ISO 8601 as stored: threads only."""

    @property
    def text(self) -> str: ...
    @property
    def resolved(self) -> bool: ...
    @property
    def replies(self) -> list[Reply]:
        """The replies of a thread, by date."""

class Reply:
    """A reply in a comment thread."""

    @property
    def author(self) -> str | None: ...
    @property
    def date(self) -> str | None: ...
    @property
    def text(self) -> str: ...

class Color:
    """A colour of a style resolved to RGB, with where it comes from."""

    @property
    def rgb(self) -> str | None:
        """``"RRGGBB"``, ``None`` for an automatic colour or one that cannot be resolved."""

    @property
    def source(self) -> Literal["rgb", "theme", "indexed", "auto"]: ...
    @property
    def index(self) -> int | None:
        """The theme index (0 ``lt1``, 1 ``dk1``, 2 ``lt2``, 3 ``dk2``, 4-9 accents, 10-11 links) or palette index."""

    @property
    def tint(self) -> float:
        """-1.0 (black) to 1.0 (white), already applied to ``rgb``."""

class Font:
    @property
    def name(self) -> str | None: ...
    @property
    def size(self) -> float | None:
        """In points."""

    @property
    def bold(self) -> bool: ...
    @property
    def italic(self) -> bool: ...
    @property
    def underline(self) -> str | None:
        """``"single"``, ``"double"``, ``"singleAccounting"`` or ``"doubleAccounting"``."""

    @property
    def strike(self) -> bool: ...
    @property
    def vertical_align(self) -> Literal["superscript", "subscript"] | None: ...
    @property
    def color(self) -> Color | None: ...

class GradientStop:
    @property
    def position(self) -> float: ...
    @property
    def color(self) -> Color | None: ...

class Fill:
    """A pattern fill (``kind == "pattern"``) or a gradient (``kind == "gradient"``)."""

    @property
    def kind(self) -> Literal["pattern", "gradient"]: ...
    @property
    def pattern(self) -> str | None:
        """``"none"``, ``"solid"``, ``"gray125"``...; ``None`` for a gradient."""

    @property
    def foreground(self) -> Color | None: ...
    @property
    def background(self) -> Color | None: ...
    @property
    def gradient_type(self) -> Literal["linear", "path"] | None: ...
    @property
    def degree(self) -> float: ...
    @property
    def left(self) -> float: ...
    @property
    def right(self) -> float: ...
    @property
    def top(self) -> float: ...
    @property
    def bottom(self) -> float: ...
    @property
    def stops(self) -> list[GradientStop]: ...

class BorderSide:
    @property
    def style(self) -> str:
        """``"none"``, ``"thin"``, ``"medium"``, ``"double"``..."""

    @property
    def color(self) -> Color | None: ...

class Border:
    @property
    def left(self) -> BorderSide: ...
    @property
    def right(self) -> BorderSide: ...
    @property
    def top(self) -> BorderSide: ...
    @property
    def bottom(self) -> BorderSide: ...
    @property
    def diagonal(self) -> BorderSide: ...
    @property
    def diagonal_up(self) -> bool: ...
    @property
    def diagonal_down(self) -> bool: ...

class Alignment:
    @property
    def horizontal(self) -> str:
        """``"general"``, ``"left"``, ``"center"``, ``"right"``, ``"fill"``, ``"justify"``..."""

    @property
    def vertical(self) -> str:
        """``"top"``, ``"center"``, ``"bottom"``, ``"justify"`` or ``"distributed"``."""

    @property
    def wrap_text(self) -> bool: ...
    @property
    def shrink_to_fit(self) -> bool: ...
    @property
    def indent(self) -> int: ...
    @property
    def text_rotation(self) -> int:
        """Degrees: 0-90 counterclockwise, 91-180 clockwise (90 less), 255 for vertical text."""

    @property
    def reading_order(self) -> int:
        """0 from the content, 1 left to right, 2 right to left."""

class Protection:
    @property
    def locked(self) -> bool: ...
    @property
    def hidden(self) -> bool: ...

class Style:
    """The style of a cell, resolved: the theme and indexed colours turned into RGB."""

    @property
    def number_format(self) -> str: ...
    @property
    def font(self) -> Font: ...
    @property
    def fill(self) -> Fill: ...
    @property
    def border(self) -> Border: ...
    @property
    def alignment(self) -> Alignment: ...
    @property
    def protection(self) -> Protection: ...
    @property
    def style_name(self) -> str | None:
        """The name of the cell style the record follows (``"Normal"``)."""

class PageMargins:
    """In inches."""

    @property
    def left(self) -> float: ...
    @property
    def right(self) -> float: ...
    @property
    def top(self) -> float: ...
    @property
    def bottom(self) -> float: ...
    @property
    def header(self) -> float: ...
    @property
    def footer(self) -> float: ...

class HeaderFooter:
    """Each header and footer with its ``&`` codes, as stored."""

    @property
    def odd_header(self) -> str | None: ...
    @property
    def odd_footer(self) -> str | None: ...
    @property
    def even_header(self) -> str | None: ...
    @property
    def even_footer(self) -> str | None: ...
    @property
    def first_header(self) -> str | None: ...
    @property
    def first_footer(self) -> str | None: ...
    @property
    def different_odd_even(self) -> bool: ...
    @property
    def different_first(self) -> bool: ...
    @property
    def scale_with_doc(self) -> bool: ...
    @property
    def align_with_margins(self) -> bool: ...

class PageBreak:
    @property
    def id(self) -> int:
        """The row or column number (from 1) the break follows."""

    @property
    def min(self) -> int: ...
    @property
    def max(self) -> int: ...
    @property
    def manual(self) -> bool: ...

class PageSetup:
    """The page setup of a sheet, schema defaults filled."""

    @property
    def margins(self) -> PageMargins | None: ...
    @property
    def paper_size(self) -> int: ...
    @property
    def paper_width(self) -> str | None: ...
    @property
    def paper_height(self) -> str | None: ...
    @property
    def orientation(self) -> Literal["default", "portrait", "landscape"]: ...
    @property
    def scale(self) -> int: ...
    @property
    def fit_to_page(self) -> bool: ...
    @property
    def fit_to_width(self) -> int: ...
    @property
    def fit_to_height(self) -> int: ...
    @property
    def first_page_number(self) -> int: ...
    @property
    def use_first_page_number(self) -> bool: ...
    @property
    def page_order(self) -> Literal["downThenOver", "overThenDown"]: ...
    @property
    def black_and_white(self) -> bool: ...
    @property
    def draft(self) -> bool: ...
    @property
    def cell_comments(self) -> Literal["none", "asDisplayed", "atEnd"]: ...
    @property
    def errors(self) -> Literal["displayed", "blank", "dash", "NA"]: ...
    @property
    def horizontal_dpi(self) -> int: ...
    @property
    def vertical_dpi(self) -> int: ...
    @property
    def copies(self) -> int: ...
    @property
    def horizontal_centered(self) -> bool: ...
    @property
    def vertical_centered(self) -> bool: ...
    @property
    def headings(self) -> bool: ...
    @property
    def grid_lines(self) -> bool: ...
    @property
    def header_footer(self) -> HeaderFooter: ...
    @property
    def row_breaks(self) -> list[PageBreak]: ...
    @property
    def column_breaks(self) -> list[PageBreak]: ...
    @property
    def print_area(self) -> str | None:
        """What the sheet's print area name refers to, as stored (``Budget!$A$1:$J$36``)."""

    @property
    def print_titles(self) -> str | None: ...
