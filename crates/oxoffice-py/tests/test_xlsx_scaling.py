"""xlsx: a read costs the cells it reads, an edit the rows it writes. A bulk read (cells, values, CSV, Markdown) of a
sheet of 4N rows, each with an array formula and its own cell style, takes about 4 times what it takes at N rows, not
16; so does the first read of a workbook of 4N cell styles, and a read of one row of 4W cells; and N single-cell reads,
writes or style edits take about the same time whatever the size of the sheet (measured on a build without debug
assertions, 2026-11-28).

The workbooks are written here: a tall sheet (N rows: a number, an array formula over its own cell, a number in one of
N cell styles, each following its own named style) and a wide one (one row of W numbers)."""
import io
import time
import zipfile

import pytest

import oxoffice.xlsx as oxlsx

N = 2000
W = 4000
CALLS = 3  # the fastest of CALLS calls counts; over 50 ms, two are enough
# A read that follows the cells measures about 4 times at 4N, one that grows faster 16 times: it fails over LINEAR
# times. A read whose cells do not grow measures about once: it fails over RATIO times. FLOOR_MS absorbs the noise of
# reads of a few milliseconds.
LINEAR, RATIO, FLOOR_MS = 8, 2.5, 5
REMEASURE = 2
MAIN = "http://schemas.openxmlformats.org/spreadsheetml/2006/main"
RELS = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"


def workbook(rows):
    """A workbook of one sheet holding `rows` (each a list of `<c>` elements without their `r`), and as many cell
    styles as rows, each following its own named style."""
    styles = max(len(rows), 1)
    sheet = "".join(
        f'<row r="{r + 1}">' + "".join(cells) + "</row>" for r, cells in enumerate(rows)
    )
    xfs = "".join(f'<xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="{i}"/>' for i in range(styles))
    parts = {
        "[Content_Types].xml": (
            '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
            '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
            '<Default Extension="xml" ContentType="application/xml"/>'
            '<Override PartName="/xl/workbook.xml" '
            'ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>'
            '<Override PartName="/xl/worksheets/sheet1.xml" '
            'ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>'
            '<Override PartName="/xl/styles.xml" '
            'ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/></Types>'
        ),
        "_rels/.rels": (
            '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
            f'<Relationship Id="rId1" Type="{RELS}/officeDocument" Target="xl/workbook.xml"/></Relationships>'
        ),
        "xl/workbook.xml": (
            f'<workbook xmlns="{MAIN}" xmlns:r="{RELS}"><sheets>'
            '<sheet name="Data" sheetId="1" r:id="rId1"/></sheets></workbook>'
        ),
        "xl/_rels/workbook.xml.rels": (
            '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
            f'<Relationship Id="rId1" Type="{RELS}/worksheet" Target="worksheets/sheet1.xml"/>'
            f'<Relationship Id="rId2" Type="{RELS}/styles" Target="styles.xml"/></Relationships>'
        ),
        "xl/worksheets/sheet1.xml": f'<worksheet xmlns="{MAIN}"><sheetData>{sheet}</sheetData></worksheet>',
        "xl/styles.xml": (
            f'<styleSheet xmlns="{MAIN}"><fonts count="1"><font><sz val="11"/><name val="Calibri"/></font></fonts>'
            '<fills count="1"><fill><patternFill patternType="none"/></fill></fills>'
            '<borders count="1"><border/></borders>'
            f'<cellStyleXfs count="{styles}">'
            + '<xf numFmtId="0" fontId="0" fillId="0" borderId="0"/>' * styles
            + f'</cellStyleXfs><cellXfs count="{styles}">{xfs}</cellXfs><cellStyles count="{styles}">'
            + "".join(f'<cellStyle name="Style {i}" xfId="{i}"/>' for i in range(styles))
            + "</cellStyles></styleSheet>"
        ),
    }
    out = io.BytesIO()
    with zipfile.ZipFile(out, "w") as package:
        for name, text in parts.items():
            package.writestr(name, text)
    return out.getvalue()


def tall(rows):
    return workbook([
        [f'<c r="A{r}"><v>{r}</v></c>', f'<c r="B{r}"><f t="array" ref="B{r}">A{r}*2</f><v>{2 * r}</v></c>',
         f'<c r="C{r}" s="{r - 1}"><v>{r}</v></c>']
        for r in range(1, rows + 1)
    ])


def wide(columns):
    return workbook([[f"<c><v>{column}</v></c>" for column in range(columns)]])


class Book:
    """A workbook read once before it is timed, so that what a timed read costs is the read itself."""

    def __init__(self, data):
        self.data = data
        self.wb = oxlsx.Workbook.from_bytes(data)
        self.ws = self.wb.sheets[0]
        self.used = self.ws.used_range


# Each read of a sized workbook, and whether its cells grow with it.
TIMED = {
    "Worksheet.cells": (lambda book: book.ws.cells(book.used), True),
    "Worksheet.values": (lambda book: book.ws.values(book.used), True),
    "Worksheet.to_csv": (lambda book: book.ws.to_csv(), True),
    "Workbook.to_markdown": (lambda book: book.wb.to_markdown(), True),
    "Workbook.from_bytes + first cell": (lambda book: oxlsx.Workbook.from_bytes(book.data).sheets[0].cell("A1"), True),
    "Worksheet.cell x N": (lambda book: [book.ws.cell((row, 1)) for row in range(N)], False),
    "Worksheet.cells, one wide row": (lambda book: book.ws.cells(book.used), True),
    "Worksheet.set_value x N": (lambda book: [book.ws.set_value((row, 3), row) for row in range(N)], False),
    "Worksheet.set_style x N": (
        lambda book: [book.ws.set_style(f"E{row + 1}", bold=row % 2 == 0) for row in range(N)],
        False,
    ),
}
WIDE = {"Worksheet.cells, one wide row"}
# ci.yml times these on every pull request and push to main.
QUICK = {
    "Worksheet.cells",
    "Workbook.from_bytes + first cell",
    "Worksheet.cell x N",
    "Worksheet.cells, one wide row",
    "Worksheet.set_value x N",
}


@pytest.fixture(scope="module")
def books():
    return {False: (Book(tall(N)), Book(tall(4 * N))), True: (Book(wide(W)), Book(wide(4 * W)))}


def costs_ms(pair, read):
    best = [float("inf")] * len(pair)
    for k in range(CALLS):
        for i, book in enumerate(pair):
            start = time.perf_counter()
            read(book)
            best[i] = min(best[i], time.perf_counter() - start)
        if k and min(best) > 0.05:
            break
    return [b * 1000 for b in best]


@pytest.mark.parametrize("name", list(TIMED))
def test_read_cost_follows_the_cells_read(books, name):
    read, grows = TIMED[name]
    bound = LINEAR if grows else RATIO
    small, large = costs_ms(books[name in WIDE], read)
    for _ in range(REMEASURE):
        if large < bound * small + FLOOR_MS:
            break
        again = costs_ms(books[name in WIDE], read)
        small, large = min(small, again[0]), min(large, again[1])
    assert large < bound * small + FLOOR_MS, f"{name}: {large:.1f} ms at 4 times the size, {small:.1f} ms"


def test_every_timed_call_runs_once():
    """Each timed read on small workbooks, untimed: what runs without OXOFFICE_SCALING."""
    small = {False: Book(tall(20)), True: Book(wide(20))}
    for name, (read, _) in TIMED.items():
        if not name.endswith(" x N"):
            read(small[name in WIDE])
    assert sorted(QUICK - set(TIMED)) == []
