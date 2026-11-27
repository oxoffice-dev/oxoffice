"""The Excel families of the corpus, written with XlsxWriter.

Each builder writes one source workbook; see common.py for the registry.
XlsxWriter writes every formula on its own: what Excel stores as one shared
formula over a filled-down column is rewritten into a shared group after the
build, in the form Excel writes it (`rewrite_part`, `share_column`).

Five sources, each feature once:

- `review`: a purchase tracker as a reviewer gets it (rows of data, one shared
  formula down a column, a total, legacy notes, a link), the workflow of
  tensorbee/rdocx#281.
- `budget`: styles and layout (fonts, fills, borders, number formats, merges,
  names, a table, validation, conditional formats, panes, a filter, print
  setup, a picture and a chart), over three sheets.
- `multi`: several sheets (one hidden) and what a cell can hold (references
  between sheets, array formulas, errors, booleans, rich text, long text,
  Unicode).
- `dates1904`: a small workbook in the 1904 date system.
- `stream`: what a streaming writer produces (XlsxWriter with `constant_memory`,
  as Apache POI's SXSSF does): inline strings, `_xHHHH_` escapes in text, and
  volatile formulas (`TODAY`, `RAND`, `OFFSET`, `INDIRECT`). No other producer
  of ours writes inline strings, and Excel is the only one that marks volatile
  formulas (`ca="1"`): its copy of this workbook is the one that shows it.

Cached values are written with every formula, computed here, so that a reader
that never recalculates shows right numbers. XlsxWriter marks every workbook
`fullCalcOnLoad`: Excel and LibreOffice recalculate on open.
"""

import random
import re
import zipfile
from datetime import datetime, timedelta
from pathlib import Path

import xlsxwriter

from common import CHART, family, sentences

CREATED = datetime(2026, 11, 21, 9, 0, 0)
AUTHOR = "Corpus Author"

# Text with accents, CJK, Arabic and an emoji, kept in the source as escapes so that the generators stay ASCII.
ACCENTS = "Caf\u00e9 cr\u00e8me br\u00fbl\u00e9e, Zo\u00eb, \u0152uvre, Stra\u00dfe, ma\u00f1ana, \u00c5ngstr\u00f6m"
CJK = "\u65e5\u672c\u8a9e\u306e\u30c6\u30ad\u30b9\u30c8\u3001\u4e2d\u6587\u6587\u672c\u3001\ud55c\uad6d\uc5b4 \ud14d\uc2a4\ud2b8"
ARABIC = "\u0645\u0631\u062d\u0628\u0627 \u0628\u0627\u0644\u0639\u0627\u0644\u0645\u060c \u0647\u0630\u0627 \u0646\u0635 \u0639\u0631\u0628\u064a"
EMOJI = "Launch \U0001f680 done \u2705"


def workbook(path: Path, title: str, **options) -> xlsxwriter.Workbook:
    """A workbook with fixed properties: the creation date is the only thing
    XlsxWriter would otherwise take from the clock."""
    book = xlsxwriter.Workbook(str(path), options)
    book.set_properties({"title": title, "author": AUTHOR, "company": "Corpus", "created": CREATED})
    return book


def rewrite_part(path: Path, name: str, edit) -> None:
    """Replace the text of one package part by `edit(text)`; every other part keeps its bytes and its place."""
    with zipfile.ZipFile(path) as package:
        parts = [(info, package.read(info)) for info in package.infolist()]
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as package:
        for info, data in parts:
            if info.filename == name:
                data = edit(data.decode("utf-8")).encode("utf-8")
            package.writestr(info, data)


def share_column(sheet_xml: str, column: str, first: int, last: int, template: str) -> str:
    """The formulas of `column` rows `first` to `last`, written one by one as
    `template` with `{r}` for the row, become one shared group: the first
    cell holds the formula and the range, the others only the group number,
    as Excel writes a formula filled down a column."""
    for row in range(first, last + 1):
        cell = rf'(<c r="{column}{row}"[^>]*>)<f>{re.escape(template.format(r=row))}</f>'
        if row == first:
            reference = f"{column}{first}:{column}{last}"
            body = f'\\1<f t="shared" ref="{reference}" si="0">{template.format(r=row)}</f>'
        else:
            body = '\\1<f t="shared" si="0"/>'
        sheet_xml, found = re.subn(cell, body, sheet_xml)
        assert found == 1, f"{column}{row}: the formula {template.format(r=row)} is not in the sheet"
    return sheet_xml


# ------------------------------------------------------------------- review

ITEMS = (
    "Standing desk", "Monitor arm", "Ergonomic chair", "Laptop dock", "Webcam", "Headset", "Desk lamp",
    "Cable tray", "Whiteboard", "Filing cabinet", "Keyboard", "Mouse", "Footrest", "Document scanner",
    "Label printer", "Network switch",
)
SUPPLIERS = ("Northwind Office", "Brightline Supply", "Harbor Equipment", "Kestrel Tech")
STATUSES = ("Ordered", "Delivered", "Needs review", "Cancelled")
REVIEW_ROWS = 36


@family("review", "xlsx")
def review(path: Path) -> None:
    """A purchase tracker with one shared formula, a total, notes and a link."""
    rng = random.Random(281)
    book = workbook(path, "Purchase tracker")
    sheet = book.add_worksheet("Orders")
    head = book.add_format({"bold": True, "bottom": 1})
    date = book.add_format({"num_format": "yyyy-mm-dd"})
    money = book.add_format({"num_format": "#,##0.00"})
    total = book.add_format({"bold": True, "top": 1})
    total_money = book.add_format({"bold": True, "top": 1, "num_format": "#,##0.00"})
    sheet.set_column("A:B", 22)
    sheet.set_column("C:C", 12)
    sheet.set_column("D:D", 10)
    sheet.set_column("E:G", 14)
    for column, name in enumerate(("Item", "Supplier", "Ordered", "Quantity", "Unit price", "Amount", "Status")):
        sheet.write(0, column, name, head)
    quantities = 0
    amounts = 0.0
    for index in range(REVIEW_ROWS):
        row, number = index + 1, index + 2
        quantity, price = rng.randint(1, 40), round(rng.uniform(8, 480), 2)
        amount = round(quantity * price, 2)
        sheet.write(row, 0, rng.choice(ITEMS))
        sheet.write(row, 1, rng.choice(SUPPLIERS))
        sheet.write_datetime(row, 2, datetime(2026, 9, 1) + timedelta(days=index), date)
        sheet.write_number(row, 3, quantity)
        sheet.write_number(row, 4, price, money)
        sheet.write_formula(row, 5, f"=D{number}*E{number}", money, amount)
        sheet.write(row, 6, rng.choices(STATUSES, weights=(5, 6, 2, 1))[0])
        quantities += quantity
        amounts += amount
    last = REVIEW_ROWS + 1
    sheet.write(last, 0, "Total", total)
    for column in (1, 2, 4, 6):
        sheet.write_blank(last, column, None, total)
    sheet.write_formula(last, 3, f"=SUM(D2:D{last})", total, quantities)
    sheet.write_formula(last, 5, f"=SUM(F2:F{last})", total_money, round(amounts, 2))
    sheet.write_url(
        last + 2, 0, "https://example.com/office-supplies/price-list", string="Supplier price list",
        tip="Open the price list",
    )
    notes = {
        "E5": "The price differs from the quote of 12 October. Please check.",
        "G9": "Status to confirm with the supplier.",
        "A14": "Is this the 140 cm model?",
    }
    for cell, text in notes.items():
        sheet.write_comment(cell, text, {"author": "Reviewer"})
    book.close()
    rewrite_part(
        path,
        "xl/worksheets/sheet1.xml",
        lambda xml: share_column(xml, "F", 2, last, "D{r}*E{r}"),
    )


# ------------------------------------------------------------------- budget

CATEGORIES = (
    # name, cost centre, headcount, change on last year, code
    ("Salaries", "Operations", 42, 18400, "GL-6010"),
    ("Rent", "Finance", 0, 0, "GL-6120"),
    ("Software", "IT", 6, 5200, "GL-6230"),
    ("Travel", "Sales", 12, -3800, "GL-6340"),
    ("Marketing", "Sales", 9, 2100, "GL-6450"),
    ("Training", "Operations", 30, -650, "GL-6560"),
    ("Equipment", "IT", 4, -9200, "GL-6670"),
    ("Utilities", "Finance", 0, 340, "GL-6780"),
)
SHOWN = ("Finance", "Operations")  # the cost centres the filter of the budget sheet keeps
TITLE_FONT = "Cambria"
EXPENSES = (
    ("Printer toner", "Equipment", 6, 54.9),
    ("Train tickets", "Travel", 14, 62.5),
    ("Conference pass", "Training", 2, 890.0),
    ("Cloud hosting", "Software", 1, 1240.0),
    ("Office plants", "Utilities", 8, 23.75),
    ("Flyers", "Marketing", 500, 0.42),
    ("Hotel nights", "Travel", 9, 138.0),
    ("Laptop sleeves", "Equipment", 12, 31.2),
    ("Language course", "Training", 4, 275.0),
    ("Licence renewal", "Software", 3, 399.0),
    ("Electricity", "Utilities", 1, 1860.45),
    ("Banner stand", "Marketing", 2, 119.0),
)
EXPENSES_FIRST_ROW = 3  # the header of the ledger table, zero-based

# Number formats of the gallery: description, code (an id for a built-in
# format), value.
GALLERY = (
    ("General", 0, 1234.5678),
    ("Thousands, two decimals", 4, 1234567.891),
    ("Percent", 9, 0.256),
    ("Percent, two decimals", 10, 0.2567),
    ("Percent, one decimal", "0.0%", 0.2567),
    ("Scientific", 11, 123456.789),
    ("Fraction, one digit", 12, 3.75),
    ("Fraction, eighths", "# ?/8", 2.375),
    ("Currency", '"$"#,##0.00', 1234.5),
    ("Currency, negative in red", '"$"#,##0.00;[Red]-"$"#,##0.00', -1234.5),
    ("Accounting", 44, 1234.5),
    ("Thousands", '#,##0,"k"', 1234567),
    ("Negative in red", "#,##0;[Red]-#,##0", -1234.5),
    ("Conditional sections", '[>=1000000]0.0,,"M";[>=1000]0.0,"k";0', 2500000),
    ("Unit", '0.0" kg"', 72.456),
    ("Zero padded", "00000", 482),
    ("Text", 49, "00123"),
    ("Date", 14, datetime(2026, 11, 21)),
    ("Date, ISO", "yyyy-mm-dd", datetime(2026, 11, 21)),
    ("Date, long", "dddd d mmmm yyyy", datetime(2026, 11, 21)),
    ("Month and year", "mmm-yy", datetime(2026, 11, 21)),
    ("Time", 21, datetime(1899, 12, 31, 14, 35, 20)),
    ("Time, 12 hours", 18, datetime(1899, 12, 31, 14, 35)),
    ("Elapsed hours", "[h]:mm", 1.0625),
    ("Date and time", 22, datetime(2026, 11, 21, 14, 35)),
)


@family("budget", "xlsx")
def budget(path: Path) -> None:
    """Styles and layout over three sheets: the budget, a ledger table and a gallery of number formats."""
    book = workbook(path, "Operating budget 2027")
    sheet = book.add_worksheet("Budget")
    ledger = book.add_worksheet("Ledger")
    gallery = book.add_worksheet("Formats")

    money = '"$"#,##0.00'
    title = book.add_format({
        "font_name": TITLE_FONT, "font_size": 16, "bold": True, "font_color": "#FFFFFF", "bg_color": "#0F766E",
        "align": "center", "valign": "vcenter",
    })
    subtitle = book.add_format({"italic": True, "font_color": "#6B7280"})
    head = book.add_format({
        "bold": True, "font_color": "#FFFFFF", "bg_color": "#1F2937", "align": "center", "valign": "vcenter",
        "text_wrap": True, "border": 1,
    })
    label = book.add_format({"indent": 1, "border": 1})
    cell = {
        "money": book.add_format({"num_format": money, "border": 1}),
        "percent": book.add_format({"num_format": "0.0%", "border": 1}),
        "change": book.add_format({"num_format": "#,##0;[Red]-#,##0", "border": 1}),
        "count": book.add_format({"num_format": 1, "border": 1}),
        "text": book.add_format({"border": 1, "align": "center"}),
    }
    total = {
        "label": book.add_format({"bold": True, "indent": 1, "top": 1, "bottom": 6}),
        "money": book.add_format({"bold": True, "num_format": money, "top": 1, "bottom": 6}),
        "percent": book.add_format({"bold": True, "num_format": "0.0%", "top": 1, "bottom": 6}),
        "change": book.add_format({"bold": True, "num_format": "#,##0;[Red]-#,##0", "top": 1, "bottom": 6}),
        "count": book.add_format({"bold": True, "num_format": 1, "top": 1, "bottom": 6}),
    }
    note = book.add_format({
        "text_wrap": True, "valign": "top", "italic": True, "font_color": "#374151", "border": 2,
        "bg_color": "#F3F4F6",
    })

    sheet.set_column("A:A", 24)
    sheet.set_column("B:F", 14)
    sheet.set_column("G:G", 10)
    sheet.set_column("H:H", 12)
    sheet.set_column("I:I", 11)
    sheet.set_column("J:J", 14)
    sheet.set_column("K:K", 10, None, {"hidden": True})
    sheet.set_row(0, 30)
    sheet.set_row(2, 32)
    sheet.merge_range("A1:J1", "Operating budget 2027", title)
    sheet.write("A2", "All amounts in dollars. Prepared by Finance, revised 21 November 2026.", subtitle)
    headers = ("Category", "Q1", "Q2", "Q3", "Q4", "Total", "Share of total", "Change on 2026", "Headcount",
               "Cost centre", "Code")
    for column, name in enumerate(headers):
        sheet.write(2, column, name, head)

    rng = random.Random(2027)
    rows = []
    for name, centre, heads, change, code in CATEGORIES:
        base = rng.randint(3, 60) * 1000
        quarters = [round(base * rng.uniform(0.9, 1.15), 2) for _ in range(4)]
        rows.append((name, quarters, round(sum(quarters), 2), centre, heads, change, code))
    grand = round(sum(row[2] for row in rows), 2)
    first, last = 4, 3 + len(rows)  # Excel rows of the data
    for index, (name, quarters, subtotal, centre, heads, change, code) in enumerate(rows):
        row, number = first - 1 + index, first + index
        sheet.write(row, 0, name, label)
        for column, value in enumerate(quarters, start=1):
            sheet.write_number(row, column, value, cell["money"])
        sheet.write_formula(row, 5, f"=SUM(B{number}:E{number})", cell["money"], subtotal)
        sheet.write_formula(row, 6, f"=F{number}/$F${last + 1}", cell["percent"], subtotal / grand)
        sheet.write_number(row, 7, change, cell["change"])
        sheet.write_number(row, 8, heads, cell["count"])
        sheet.write(row, 9, centre, cell["text"])
        sheet.write(row, 10, code)
        if centre not in SHOWN:
            sheet.set_row(row, None, None, {"hidden": True})
    sheet_total = last  # the total row, zero-based: row `last + 1` in Excel
    sums = [round(sum(row[1][quarter] for row in rows), 2) for quarter in range(4)]
    sheet.write(sheet_total, 0, "Total", total["label"])
    for column, value in enumerate(sums, start=1):
        letter = "BCDE"[column - 1]
        sheet.write_formula(sheet_total, column, f"=SUM({letter}{first}:{letter}{last})", total["money"], value)
    sheet.write_formula(sheet_total, 5, "=SUM(Totals)", total["money"], grand)
    sheet.write_formula(sheet_total, 6, f"=SUM(G{first}:G{last})", total["percent"], 1)
    sheet.write_formula(sheet_total, 7, f"=SUM(H{first}:H{last})", total["change"], sum(row[5] for row in rows))
    sheet.write_formula(sheet_total, 8, f"=SUM(I{first}:I{last})", total["count"], sum(row[4] for row in rows))
    tax = 0.2
    sheet.write(sheet_total + 1, 0, "Total with tax", total["label"])
    sheet.write_formula(sheet_total + 1, 5, "=F12*(1+TaxRate)", total["money"], round(grand * (1 + tax), 2))
    sheet.write(14, 0, "Tax rate", label)
    sheet.write_number(14, 1, tax, book.add_format({"num_format": 9, "border": 1}))
    sheet.merge_range(
        "A17:J19",
        "Salaries follow the hiring plan agreed in October. Travel and equipment are capped until the "
        "second quarter; any overspend needs the approval of the steering committee.",
        note,
    )
    for row in (16, 17, 18):
        sheet.set_row(row, 20)

    book.define_name("TaxRate", "=Budget!$B$15")
    book.define_name("Budget!Totals", f"=Budget!$F${first}:$F${last}")

    sheet.data_validation(f"I{first}:I{last}", {
        "validate": "integer", "criteria": "between", "minimum": 0, "maximum": 500,
        "error_title": "Headcount", "error_message": "Enter a whole number from 0 to 500.",
    })
    sheet.data_validation(f"J{first}:J{last}", {
        "validate": "list", "source": ["Finance", "Operations", "Sales", "IT"],
        "input_title": "Cost centre", "input_message": "Pick a cost centre from the list.",
    })
    sheet.conditional_format(f"H{first}:H{last}", {
        "type": "cell", "criteria": "<", "value": 0,
        "format": book.add_format({"bg_color": "#FEE2E2", "font_color": "#991B1B"}),
    })
    sheet.conditional_format(f"G{first}:G{last}", {
        "type": "3_color_scale", "min_color": "#FFFFFF", "mid_color": "#FDE68A", "max_color": "#F59E0B",
    })
    sheet.conditional_format(f"F{first}:F{last}", {"type": "data_bar", "bar_color": "#5EEAD4", "data_bar_2010": True})

    sheet.freeze_panes(3, 1)
    sheet.autofilter(f"A3:J{last}")
    sheet.filter_column_list(9, list(SHOWN))

    chart = book.add_chart({"type": "column"})
    chart.add_series({
        "name": "Total by quarter", "categories": "=Budget!$B$3:$E$3", "values": f"=Budget!$B${last + 1}:$E${last + 1}",
        "fill": {"color": "#0F766E"},
    })
    chart.set_title({"name": "Spending by quarter"})
    chart.set_legend({"none": True})
    chart.set_size({"width": 520, "height": 300})
    sheet.insert_chart("A21", chart)
    sheet.insert_image("G21", str(CHART), {"x_scale": 0.35, "y_scale": 0.35, "description": "Spending by quarter, 2026"})

    sheet.set_landscape()
    sheet.set_paper(9)
    sheet.fit_to_pages(1, 0)
    sheet.print_area("A1:J36")
    sheet.repeat_rows(2)
    sheet.set_margins(left=0.5, right=0.5, top=0.75, bottom=0.75)
    sheet.center_horizontally()
    sheet.set_header('&L&"Calibri,Bold"Operating budget 2027&RConfidential')
    sheet.set_footer("&LPrepared by Finance&CPage &P of &N")

    # The ledger: a table with a calculated column and a total row.
    ledger.write("A1", "Ledger, fourth quarter 2026", book.add_format({"bold": True, "font_size": 14}))
    date, price = book.add_format({"num_format": "yyyy-mm-dd"}), book.add_format({"num_format": money})
    rng = random.Random(1126)
    data, amounts, units = [], [], 0
    for index, (what, category, quantity, unit) in enumerate(EXPENSES):
        data.append([datetime(2026, 10, 1) + timedelta(days=2 * index + rng.randint(0, 1)), category, what, quantity, unit, None])
        amounts.append(round(quantity * unit, 2))
        units += quantity
    last_row = EXPENSES_FIRST_ROW + len(data) + 1  # the total row, zero-based
    ledger.add_table(EXPENSES_FIRST_ROW, 0, last_row, 5, {
        "name": "Expenses", "style": "Table Style Medium 2", "total_row": True, "data": data,
        "columns": [
            {"header": "Date", "format": date, "total_string": "Total"},
            {"header": "Category"},
            {"header": "Description"},
            {"header": "Quantity", "total_function": "sum", "total_value": units},
            {"header": "Unit price", "format": price},
            {"header": "Amount", "format": price, "formula": "=Expenses[[#This Row],[Quantity]]*Expenses[[#This Row],[Unit price]]",
             "total_function": "sum", "total_value": round(sum(amounts), 2)},
        ],
    })
    for index, amount in enumerate(amounts):
        ledger.write_formula(
            EXPENSES_FIRST_ROW + 1 + index, 5, "=Expenses[[#This Row],[Quantity]]*Expenses[[#This Row],[Unit price]]",
            price, amount,
        )
    ledger.set_column("A:A", 12)
    ledger.set_column("B:B", 12)
    ledger.set_column("C:C", 20)
    ledger.set_column("D:F", 12)

    # The gallery: one value per number format, with its code beside it.
    gallery.set_column("A:A", 28)
    gallery.set_column("B:B", 44)
    gallery.set_column("C:C", 26)
    for column, name in enumerate(("Format", "Code", "Value")):
        gallery.write(0, column, name, book.add_format({"bold": True, "bottom": 1}))
    for index, (what, code, value) in enumerate(GALLERY, start=1):
        shown = book.add_format({"num_format": code, "align": "right"})
        gallery.write(index, 0, what)
        gallery.write_string(index, 1, code if isinstance(code, str) else f"built-in {code}")
        if isinstance(value, datetime):
            gallery.write_datetime(index, 2, value, shown)
        elif isinstance(value, str):
            gallery.write_string(index, 2, value, shown)
        else:
            gallery.write_number(index, 2, value, shown)
    book.close()


# -------------------------------------------------------------------- multi

REGIONS = (("North", 0.05), ("South", 0.04), ("East", 0.06), ("West", 0.045))
PRODUCTS = (("Router", 120, 89.0), ("Switch", 80, 149.5), ("Cable pack", 640, 12.25),
            ("Access point", 55, 210.0), ("Firewall", 18, 899.0), ("Rack", 12, 420.0))
TARGET = 45000
LONG_TEXT = "\n".join(
    "Paragraph {n}: the regional teams reviewed the quarterly figures together with the finance department, "
    "checked every adjustment against the ledger, and agreed that the remaining differences come from timing, "
    "not from errors; the notes below list what was decided, who owns each action and when it is due.".format(n=n)
    for n in range(1, 11)
)


@family("multi", "xlsx")
def multi(path: Path) -> None:
    """Five sheets (one hidden) with references between them and every kind of cell value."""
    book = workbook(path, "Regional sales")
    summary = book.add_worksheet("Summary")
    sales = book.add_worksheet("Sales")
    forecast = book.add_worksheet("Q1 Forecast")
    rates = book.add_worksheet("Rates")
    values = book.add_worksheet("Values")
    rates.hide()
    summary.set_tab_color("#0F766E")

    head = book.add_format({"bold": True, "bottom": 1})
    date = book.add_format({"num_format": "mmm yyyy"})
    money = book.add_format({"num_format": "#,##0.00"})
    percent = book.add_format({"num_format": "0.0%"})
    bold = book.add_format({"bold": True})

    # Sales: four regions over six months.
    rng = random.Random(1904)
    sales.set_column("A:A", 10)
    sales.set_column("B:B", 12)
    sales.set_column("C:E", 12)
    for column, name in enumerate(("Region", "Month", "Units", "Revenue", "Shipped")):
        sales.write(0, column, name, head)
    units, revenue = {}, {}
    row = 1
    for region, _ in REGIONS:
        price = round(rng.uniform(20, 60), 2)
        for month in range(6):
            count = rng.randint(80, 400)
            amount = round(count * price, 2)
            sales.write(row, 0, region)
            sales.write_datetime(row, 1, datetime(2026, 1 + month, 1), date)
            sales.write_number(row, 2, count)
            sales.write_number(row, 3, amount, money)
            sales.write_boolean(row, 4, month < 5)
            units[region] = units.get(region, 0) + count
            revenue[region] = round(revenue.get(region, 0) + amount, 2)
            row += 1
    last = row  # the last data row, in Excel numbering

    # Rates: hidden, looked up by the summary.
    rates.write_row(0, 0, ("Region", "Commission"), head)
    for index, (region, rate) in enumerate(REGIONS, start=1):
        rates.write(index, 0, region)
        rates.write_number(index, 1, rate, percent)

    # Q1 Forecast: array formulas, one of them dynamic.
    forecast.set_column("A:A", 14)
    forecast.set_column("F:F", 14)
    forecast.set_column("H:H", 18)
    forecast.write_row(0, 0, ("Product", "Units", "Price", "Revenue"), head)
    forecast.write(0, 5, "Units, largest first", head)
    forecast.write(0, 7, "Target per region", head)
    forecast.write_number(1, 7, TARGET, money)
    for index, (name, count, price) in enumerate(PRODUCTS, start=1):
        forecast.write(index, 0, name)
        forecast.write_number(index, 1, count)
        forecast.write_number(index, 2, price, money)
    products = [round(count * price, 2) for _, count, price in PRODUCTS]
    forecast.write_array_formula("D2:D7", "{=B2:B7*C2:C7}", money, products[0])
    for index, amount in enumerate(products[1:], start=2):
        forecast.write_number(index, 3, amount, money)
    forecast.write("A8", "Total", bold)
    forecast.write_array_formula("D8:D8", "{=SUM(B2:B7*C2:C7)}", book.add_format({"bold": True, "num_format": "#,##0.00"}),
                                 round(sum(products), 2))
    ranked = sorted((count for _, count, _ in PRODUCTS), reverse=True)
    forecast.write_dynamic_array_formula("F2:F7", "=_xlfn._xlws.SORT(B2:B7,,-1)", None, ranked[0])
    for index, count in enumerate(ranked[1:], start=2):
        forecast.write_number(index, 5, count)

    # Summary: formulas across sheets, and their results.
    summary.set_column("A:A", 12)
    summary.set_column("B:F", 13)
    summary.set_column("G:G", 24)
    summary.write("A1", "Regional sales summary", book.add_format({"bold": True, "font_size": 14}))
    for column, name in enumerate(("Region", "Units", "Revenue", "Commission rate", "Commission", "Target met", "Label")):
        summary.write(2, column, name, head)
    for index, (region, rate) in enumerate(REGIONS):
        row, number = 3 + index, 4 + index
        summary.write(row, 0, region)
        summary.write_formula(row, 1, f"=SUMIF(Sales!$A$2:$A${last},A{number},Sales!$C$2:$C${last})", None, units[region])
        summary.write_formula(row, 2, f"=SUMIF(Sales!$A$2:$A${last},A{number},Sales!$D$2:$D${last})", money, revenue[region])
        summary.write_formula(row, 3, f"=VLOOKUP(A{number},Rates!$A$2:$B$5,2,FALSE)", percent, rate)
        summary.write_formula(row, 4, f"=C{number}*D{number}", money, round(revenue[region] * rate, 2))
        summary.write_formula(row, 5, f"=C{number}>='Q1 Forecast'!$H$2", None, revenue[region] >= TARGET)
        summary.write_formula(
            row, 6, f'=A{number}&": "&TEXT(C{number},"#,##0")', None, f"{region}: {revenue[region]:,.0f}",
        )
    summary.write("A8", "Total", bold)
    summary.write_formula("B8", "=SUM(B4:B7)", bold, sum(units.values()))
    summary.write_formula("C8", "=SUM(C4:C7)", book.add_format({"bold": True, "num_format": "#,##0.00"}),
                          round(sum(revenue.values()), 2))
    summary.write_url("A10", "internal:'Q1 Forecast'!A1", string="Go to the forecast")

    # Values: what a cell can hold.
    values.set_column("A:A", 24)
    values.set_column("B:B", 70)
    values.write_row(0, 0, ("Kind", "Value"), head)
    errors = (
        ("#DIV/0!", "=1/0"), ("#N/A", "=NA()"), ("#VALUE!", '="a"+1'), ("#REF!", "=#REF!"),
        ("#NAME?", "=MissingName"), ("#NUM!", "=SQRT(-1)"), ("#NULL!", "=SUM(A2:A3 B2:B3)"),
    )
    row = 1
    for code, formula in errors:
        values.write(row, 0, f"Error {code}")
        values.write_formula(row, 1, formula, None, code)
        row += 1
    values.write(row, 0, "Boolean true")
    values.write_boolean(row, 1, True)
    values.write(row + 1, 0, "Boolean false")
    values.write_boolean(row + 1, 1, False)
    values.write(row + 2, 0, "Boolean formula")
    values.write_formula(row + 2, 1, "=1<2", None, True)
    row += 3
    bold_run, italic_run = book.add_format({"bold": True}), book.add_format({"italic": True})
    red_run = book.add_format({"bold": True, "font_color": "#B91C1C", "font_size": 14})
    values.write(row, 0, "Rich text")
    values.write_rich_string(row, 1, "Plain, ", bold_run, "bold", " and ", italic_run, "italic", ", then ", red_run, "red and large")
    values.write(row + 1, 0, "Long text")
    values.write_string(row + 1, 1, LONG_TEXT, book.add_format({"text_wrap": True, "valign": "top"}))
    values.write(row + 2, 0, "Spaces")
    values.write_string(row + 2, 1, "  two spaces before and after  ")
    values.write(row + 3, 0, "Escapes")
    values.write_string(row + 3, 1, "Tom & Jerry <\"fun\"> it's 5 > 3")
    values.write(row + 4, 0, "Accents")
    values.write_string(row + 4, 1, ACCENTS)
    values.write(row + 5, 0, "CJK")
    values.write_string(row + 5, 1, CJK)
    values.write(row + 6, 0, "Arabic")
    values.write_string(row + 6, 1, ARABIC, book.add_format({"reading_order": 2, "align": "right"}))
    values.write(row + 7, 0, "Emoji")
    values.write_string(row + 7, 1, EMOJI)
    book.close()


# ---------------------------------------------------------------- dates1904


@family("dates1904", "xlsx")
def dates1904(path: Path) -> None:
    """Dates, times and a difference in the 1904 date system, where serial 0 is 1 January 1904."""
    book = workbook(path, "Dates in the 1904 system", date_1904=True)
    sheet = book.add_worksheet("Dates")
    head = book.add_format({"bold": True, "bottom": 1})
    day = book.add_format({"num_format": "yyyy-mm-dd"})
    clock = book.add_format({"num_format": "hh:mm:ss"})
    stamp = book.add_format({"num_format": "yyyy-mm-dd hh:mm"})
    sheet.set_column("A:A", 22)
    sheet.set_column("B:C", 18)
    sheet.write_row(0, 0, ("Description", "Value", "Plus 30 days"), head)
    rows = (
        ("Serial zero", datetime(1904, 1, 1)),
        ("A leap day", datetime(2000, 2, 29)),
        ("Review date", datetime(2026, 11, 21)),
    )
    for index, (what, when) in enumerate(rows, start=1):
        sheet.write(index, 0, what)
        sheet.write_datetime(index, 1, when, day)
        sheet.write_formula(index, 2, f"=B{index + 1}+30", day, (when - datetime(1904, 1, 1)).days + 30)
    sheet.write(4, 0, "Time of day")
    sheet.write_datetime(4, 1, datetime(1904, 1, 1, 13, 45, 30), clock)
    sheet.write(5, 0, "Date and time")
    sheet.write_datetime(5, 1, datetime(2026, 11, 21, 14, 35), stamp)
    sheet.write(6, 0, "Days between")
    sheet.write_formula(6, 1, "=B4-B2", None, (rows[2][1] - rows[0][1]).days)
    book.close()


# ------------------------------------------------------------------- stream

LOG_ROWS = 24
LOG_SOURCES = ("scheduler", "importer", "mailer", "exporter")
LOG_LEVELS = ("info", "info", "info", "warning", "error")


@family("stream", "xlsx")
def stream(path: Path) -> None:
    """A log as a streaming writer saves it: inline strings, `_xHHHH_` escapes, volatile formulas."""
    rng = random.Random(365)
    book = workbook(path, "Import log", constant_memory=True)  # rows in order, strings inline
    sheet = book.add_worksheet("Log")
    head = book.add_format({"bold": True, "bottom": 1})
    when = book.add_format({"num_format": "yyyy-mm-dd hh:mm"})
    day = book.add_format({"num_format": "yyyy-mm-dd"})
    number = book.add_format({"num_format": "#,##0.00"})
    sheet.set_column("A:A", 18)
    sheet.set_column("B:C", 12)
    sheet.set_column("D:D", 70)
    sheet.set_column("E:E", 12)
    sheet.write_row(0, 0, ("Time", "Source", "Level", "Message", "Value"), head)
    values = []
    for index, message in enumerate(sentences(365, LOG_ROWS)):
        row = index + 1
        sheet.write_datetime(row, 0, datetime(2026, 11, 20, 8, 0) + timedelta(minutes=20 * index), when)
        sheet.write(row, 1, rng.choice(LOG_SOURCES))
        sheet.write(row, 2, rng.choice(LOG_LEVELS))
        if index == 0:
            sheet.write_rich_string(row, 3, "Started ", book.add_format({"bold": True}), "the nightly import", ". ", message)
        else:
            sheet.write(row, 3, message)
        values.append(round(rng.uniform(1, 5000), 2))
        sheet.write_number(row, 4, values[-1], number)
    # Volatile formulas, with the value they had on the day the sources were written (21 November 2026).
    first = LOG_ROWS + 3
    sheet.write(first - 1, 0, "Volatile", head)
    cached = (
        ("Today", "=TODAY()", 46347, day),
        ("Random", "=RAND()", 0.25, None),
        ("Next five values", "=SUM(OFFSET(E2,0,0,5,1))", round(sum(values[:5]), 2), number),
        ("Last value", f'=INDIRECT("E"&COUNTA(D1:D{LOG_ROWS + 1}))', values[-1], number),
    )
    for index, (label, formula, value, style) in enumerate(cached):
        sheet.write(first + index, 0, label)
        sheet.write_formula(first + index, 1, formula, style, value)
    # Text as Excel stores it: a line break as `_x000D_` plus a line feed, a literal `_x0041_` as `_x005F_x0041_`.
    sheet.write(first + 5, 0, "Line break")
    sheet.write_string(first + 5, 3, "First line\r\nsecond line")
    sheet.write(first + 6, 0, "Escape sequence")
    sheet.write_string(first + 6, 3, "The text _x0041_ is not the letter A")
    book.close()
