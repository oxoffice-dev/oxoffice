"""The cases of tests/cases.tsv, grouped by the Excel round that checked them.

Writes the cases of one round into an output directory:
- cases-rN.tsv: id, code, value, date_system, kind (one row per case);
- cases-rN-1900.xlsx and cases-rN-1904.xlsx: the cases of each date system (a workbook setting), sheet
  "cases": A id, B code (text), C the value written with the code applied (the cell Excel reads), D the
  kind. Column C is 40 characters wide.

Conventions:
- code `builtin:N` is the built-in format id N, written as numFmtId N with no numFmt record;
- kind `number` (value is a decimal literal), `text` (value is the string), `bool` (TRUE or FALSE);
- values have at most 15 significant digits, so the file holds the same double.

A new round adds its cases at the end, after `ROUND = N`. See README.md.

Run: uv run --with xlsxwriter python cases.py <out-dir> <round>
"""

import datetime
import pathlib
import sys

import xlsxwriter

def serial(y, mo, d, h=0, mi=0, s=0.0, system=1900):
    """The serial of a date in the 1900 (from 1900-03-01 on) or 1904 system."""
    base = datetime.datetime(1899, 12, 30) if system == 1900 else datetime.datetime(1904, 1, 1)
    delta = datetime.datetime(y, mo, d) - base
    return delta.days + (h * 3600 + mi * 60 + s) / 86400


def lit(x):
    """A decimal literal of at most 15 significant digits."""
    if isinstance(x, str):
        return x
    text = f"{x:.15g}"
    return text


D = serial(2024, 1, 15)  # a Monday
DT = serial(2024, 1, 15, 14, 30, 45)  # 45306.6046875
PM5 = serial(2024, 1, 15, 17, 5, 9)

CASES = []  # (id, code, value, system, kind, round)
ROUND = 1


def add(group, code, *values, system=1900, kind="number"):
    for value in values:
        n = sum(1 for c in CASES if c[0].startswith(group + "-")) + 1
        CASES.append((f"{group}-{n:02d}", code, lit(value), str(system), kind, ROUND))


def text(group, code, *values, system=1900):
    add(group, code, *values, system=system, kind="text")


# General: 11 characters, scientific switch, no trailing zeros.
add("gen", "General", "0", "1", "-1", "0.5", "0.1", "-0.5", "-0")
add("gen", "General", "1234567890", "12345678901", "123456789012", "-12345678901", "-1234567890")
add("gen", "General", "99999999999", "99999999999.5", "100000000000", "1.23456789012346E+19")
add("gen", "General", "0.333333333333333", "0.666666666666667", "-0.333333333333333")
add("gen", "General", "123.456789012345", "123456.789012", "1234567.891", "12345678.91", "123456789.12")
add("gen", "General", "0.123456789012", "-0.123456789012", "3.14159265358979")
add("gen", "General", "0.001", "0.0001", "0.00001", "0.000001", "0.0000001", "0.0000123", "0.000123456789")
add("gen", "General", "1E-9", "1E-10", "1.5E-10", "-1.5E-10", "1E-100", "2.2250738585072E-308")
add("gen", "General", "1E+15", "1E+20", "1.5E+300", "1E+307", "9007199254740992", "1.23456789012E+15")
add("gen", "General", "0.3", "1234567890.5", "-1234567890.5", "1.0000000001")
text("gen", "General", "abc", "123", "")

# Digit placeholders, decimal point, thousands separator, rounding.
add("num", "0", "0", "0.4", "0.5", "1.5", "2.5", "-0.4", "-0.5", "-2.5", "123456789012345000")
add("num", "0.00", "1.005", "2.675", "0.005", "0.015", "-0.005", "-0.001", "-0", "1E-20", "1234.5")
add("num", "0.0", "9.95", "1E+20", "-9.95")
add("num", "#", "0", "5", "0.4")
add("num", "#.##", "0", "0.001", "5", "1.234")
add("num", "0.##", "5", "1.005", "0.5")
add("num", "#,##0", "1234567", "-1234567", "0", "999.5", "1234567890123")
add("num", "#,##0.00", "-1234.567", "1E+15", "0.004")
add("num", "#,###", "0", "1234")
add("num", "00000", "123", "-123", "123456")
add("num", "0.0#", "1.5", "1.234", "1")
add("num", "???.???", "1.5", "-12.25", "0")
add("num", "?", "0", "5")
add("num", "0.00000000000000000000", "0.333333333333333", "0.1")
add("num", "000.000", "1.5")
add("num", "#,##0.00_);(#,##0.00)", "1234.5", "-1234.5", "0")
add("num", "0.00;(0.00);0.00", "-0.001")
add("num", "#,#0", "123456")
add("num", "0,000", "5", "1234567")
add("num", "#0.0,0", "1234.5")

# Scaling by trailing commas and percent.
add("scale", "#,##0,", "1234567", "499", "500")
add("scale", "0.0,\"K\"", "1234567")
add("scale", "0.0,,\"M\"", "1234567", "-1234567")
add("scale", "#,##0,,", "1234567890")
add("scale", "#,", "999", "400")
add("scale", "0%", "0.256", "1.5", "-0.004", "0")
add("scale", "0.0%", "0.0005", "-0.001234")
add("scale", "0.00%", "1")
add("scale", "#,##0.0%", "123.456")
add("scale", "0%%", "0.5")
add("scale", "0.0,%", "1234")

# Exponents.
add("exp", "0.00E+00", "12345", "0.000123", "0", "-12345", "9.995", "1E+300", "1E-300", "1", "-0.000123")
add("exp", "0.00E-00", "12345", "0.000123")
add("exp", "0E+0", "12345", "0.5")
add("exp", "##0.0E+0", "12345", "0.00123", "1234567", "0.5", "100")
add("exp", "#0.0E+0", "12345")
add("exp", "0.0e+00", "12345")
add("exp", "0.00E+000", "1E+100", "1.5E-5")
add("exp", "#,##0.00E+00", "12345")
add("exp", "00.00E+00", "12345")
add("exp", "0.00E+00;(0.00E+00)", "-12345")

# Fractions.
add("frac", "# ?/?", "1.5", "0.5", "0", "1.0001", "-1.5", "0.666666666666667", "5", "0.99")
add("frac", "# ??/??", "3.14159265358979", "0.333333333333333", "-0.25", "123.456")
add("frac", "# ??/16", "1.3", "0.0625", "2.99")
add("frac", "?/?", "1.5", "0.1")
add("frac", "# ?/4", "0.3", "2.9", "2.1")
add("frac", "# ?/8", "0.75")
add("frac", "?/???", "0.333")
add("frac", "0 ?/?", "0.5", "0")
add("frac", "# ???/???", "0.123456")
add("frac", "?? ?/?", "1.5")
add("frac", "#\" \"?/?", "2.5")
add("frac", "# ?/10", "0.37")
add("frac", "# ?/?;(# ?/?)", "-2.5")
add("frac", "0.0 ?/?", "1.5")

# Sections, colours and conditions.
add("sec", "0.00;(0.00)", "-5", "5", "0")
add("sec", "0.00;-0.00", "-5")
add("sec", "0;0", "-5")
add("sec", "0;-0;;@", "0", "5")
text("sec", "0;-0;;@", "abc")
add("sec", "0;-0;\"zero\"", "0", "-0.4")
add("sec", "0;(0);\"zero\";\"text: \"@", "5", "-5", "0")
text("sec", "0;(0);\"zero\";\"text: \"@", "abc")
add("sec", "0.0;[Red]-0.0", "-5", "5")
add("sec", "[Red]0;[Blue]-0;[Green]0;[Magenta]@", "5", "-5", "0")
add("sec", "[Color10]0.00", "5")
add("sec", "[color 45]0", "5")
add("sec", "[BLACK]0;[White]-0;[Cyan]0;[Yellow]@", "-1")
add("sec", "[>=100]\"big\";[<0]\"neg\";\"small\"", "150", "-5", "50", "100")
add("sec", "[>=100]0.00;0", "99.999", "100", "-5")
add("sec", "[<=-100]\"low\";[>=100]\"high\";0", "-200", "200", "-5", "5")
add("sec", "[>0]\"pos\";[>10]\"big\";\"other\"", "20", "-3", "0")
add("sec", "[>100]0;0", "-5", "200")
add("sec", "[<-10]\"a\";[<0]\"b\";0", "-20", "-5", "5")
add("sec", "[=0]\"zero\";0", "0", "-3", "3")
add("sec", "[>=100]0", "50", "150", "-150")
add("sec", "[<0]\"neg\"", "5")
add("sec", "[Red][<=100]0;[Blue][>100]0", "50", "150")
add("sec", "[<>5]0;\"five\"", "5", "6")
add("sec", "[>=1E3]0,\"K\";0", "12345", "999")
add("sec", "0;\"neg\"", "-5", "0")
add("sec", "0;-0;0;@", "-0")
add("sec", ";;;", "5", "-5", "0")
text("sec", ";;;", "abc")
add("sec", "0;-0;\"-\"", "0")
add("sec", "0.00;;", "-5", "0")
add("sec", ";0", "5", "-5")
add("sec", "#,##0;-#,##0;0;\"[\"@\"]\"", "-1234")
text("sec", "#,##0;-#,##0;0;\"[\"@\"]\"", "x")

# Text values and the @ section.
text("text", "@", "abc", "0012", "")
add("text", "@", "1234.5", "0.1", "123456789012")
text("text", "@\" suffix\"", "x")
text("text", "\"pre \"@", "x")
add("text", "\"pre \"@", "5")
text("text", "@@", "ab")
text("text", "0.00", "abc", "123")
text("text", "0;0;0", "abc")
text("text", "m/d/yyyy", "abc")
text("text", "0;-0;0;", "abc")
add("bool", "0.00", "TRUE", kind="bool")
add("bool", "General", "FALSE", kind="bool")

# Literals, escapes, spaces and fills.
add("lit", "\"$\"#,##0.00", "1234.567")
add("lit", "\\$0.00", "5")
add("lit", "$#,##0.00", "-1234.567")
add("lit", "0\" units\"", "5")
add("lit", "0 \"kg\"", "5")
add("lit", "0\\ \\k\\g", "5")
add("lit", "\\#0", "5")
add("lit", "0\\\"", "5")
add("lit", "(0)", "5")
add("lit", "+0;-0", "5", "-5")
add("lit", "0.0 \"°C\"", "21.5")
add("lit", "0-0-0", "123")
add("lit", "000-00-0000", "123456789")
add("lit", "(000) 000-0000", "5551234567")
add("lit", "0!x", "5")
add("lit", "0_)", "5")
add("lit", "_(0_)", "5")
add("lit", "0__", "5")
add("lit", "*-0", "5")
add("lit", "0*-", "5")
add("lit", "* #,##0", "1234")
add("lit", "\"x\"*.0", "5")
text("lit", "@*.", "abc")
add("lit", "_($* #,##0.00_);_($* (#,##0.00);_($* \"-\"??_);_(@_)", "1234.5", "-1234.5", "0")
text("lit", "_($* #,##0.00_);_($* (#,##0.00);_($* \"-\"??_);_(@_)", "abc")
add("lit", "0.00\" \"\"x\"", "5")
add("lit", "0.0 %", "0.5")
add("lit", "#\"#\"#", "123")
add("lit", "0 ^ ~ { } < > = & '", "5")

# Locale and currency tags.
add("loc", "[$$-409]#,##0.00", "1234.567", "-1234.567")
add("loc", "[$€-40C]#,##0.00", "1234.567")
add("loc", "#,##0.00 [$€-40C]", "1234.567")
add("loc", "#,##0.00\\ [$€-1]", "1234.567")
add("loc", "[$£-809]#,##0.00", "1234.567")
add("loc", "[$USD] #,##0.00", "1234.567")
add("loc", "[$-409]#,##0.00", "1234.567")
add("loc", "[$¥-411]#,##0", "1234")
add("loc", "[$-409]mmmm d, yyyy", D)
add("loc", "[$-409]h:mm:ss AM/PM", DT)
add("loc", "[$-409]m/d/yy h:mm AM/PM;@", DT)
add("loc", "[$-F800]dddd, mmmm dd, yyyy", D)
add("loc", "[$-F400]h:mm:ss AM/PM", DT)
add("loc", "[$-40C]mmmm", D)
add("loc", "[$-407]dddd", D)
add("loc", "[$-x-sysdate]dddd, mmmm dd, yyyy", D)
add("loc", "[$-en-US]mmmm", D)
add("loc", "[$-10409]0.00", "5")

# Dates and times (1900 system).
add("date", "m/d/yyyy", D, "1", "59", "60", "61", "0", "367", "2958465", "2958466", "-1", "0.5")
add("date", "m/d/yyyy", "45306.9999999", "45306.99999999", "45306.5")
add("date", "d-mmm-yy", D)
add("date", "d-mmm", D)
add("date", "mmm-yy", D)
add("date", "mmmm d, yyyy", D)
add("date", "mmmmm", D, serial(2024, 2, 1))
add("date", "dddd", D, "1", "60", "61", "0")
add("date", "ddd", D)
add("date", "dd/mm/yyyy", D)
add("date", "yyyy-mm-dd", D, "0", "60")
add("date", "yyyy-mm-dd hh:mm:ss", DT, "45306.99999999", "45306.999994")
add("date", "m/d/yyyy h:mm", DT, "0.5")
add("date", "d", D, "60")
add("date", "dd", serial(2024, 1, 5))
add("date", "m", D)
add("date", "mm", D)
add("date", "mmm", "60")
add("date", "y", D)
add("date", "yy", D)
add("date", "yyy", D)
add("date", "yyyy", D, "1")
add("date", "yyyyy", D)
add("date", "ddd, mmm d", D)
add("date", "d/m/yy h:mm", PM5)
add("date", "mm/dd/yy hh:mm", PM5)
add("date", "e", D)
add("date", "dd.mm.yyyy", D)
add("date", "dddd mm/dd/yyyy", "1", "59", "60", "61")
add("date", "m/d/yyyy", "1E+20")
add("date", "yyyy-mm-dd", "-0.5")
add("time", "h:mm", DT, D, "0.999999")
add("time", "h:mm AM/PM", DT, D, "0.5", PM5)
add("time", "h:mm:ss A/P", DT, "0.25")
add("time", "h:mm:ss a/p", DT)
add("time", "hh:mm am/pm", DT)
add("time", "h AM/PM", "0", "0.5", "0.999")
add("time", "hh:mm:ss", DT, "0.99999999", "0.500005787037037", "0.500005775")
add("time", "h:mm:ss", "0.99999999", "0.500005787037037", "1.5")
add("time", "h:mm:ss AM/PM", "0.99999999")
add("time", "hh:mm:ss.000", "45306.6046875116")
add("time", "h:mm:ss.0", "0.500006365740741")
add("time", "h:mm:ss.00", "0.500006365740741")
add("time", "mm:ss", "0.0104166666666667")
add("time", "m:ss", "0.0104166666666667")
add("time", "mm:ss.0", "0.0000115740740740741", "0.000694212962962963")
add("time", "ss.0", "0.000694212962962963")
add("time", "[h]:mm:ss", "1.5", "2.75", "0", "1", "25.5", "-1.5", "0.99999999")
add("time", "[hh]:mm", "0.25", "1.25")
add("time", "[h]:mm", "25.5")
add("time", "[mm]:ss", "0.0625", "1")
add("time", "[m]", "1")
add("time", "[ss]", "0.01")
add("time", "[s].00", "0.0000115740740740741")
add("time", "h:m", DT, PM5)
add("time", "h \"h\" m \"m\"", PM5)
add("time", "mm ss", PM5)
add("time", "yyyy mm", D)
add("time", "mm yyyy", D)
add("time", "m s", PM5)
add("time", "hh:mm:ss;@", "-0.5")
add("time", "[h]:mm:ss.0", "1.00000063657407")
add("time", "h:mm;[Red]-h:mm", "-0.5")

# Built-in ids, as stored with no numFmt record (en-US Excel shows its own codes).
for n in [1, 2, 3, 4, 9, 10, 11, 12, 13, 37, 38, 39, 40, 48]:
    add("builtin", f"builtin:{n}", "-1234.567")
for n in [5, 6, 7, 8, 41, 42, 43, 44]:
    add("builtin", f"builtin:{n}", "-1234.567", "0")
for n in [14, 15, 16, 17, 18, 19, 20, 21, 22, 45, 46, 47]:
    add("builtin", f"builtin:{n}", DT)
for n in list(range(23, 37)) + list(range(50, 59)) + [59, 62, 67, 69, 71, 76, 79, 81]:
    add("builtin", f"builtin:{n}", DT)
add("builtin", "builtin:49", "1234.5")
add("builtin", "builtin:46", "1.5")

# 1904 date system.
add("d1904", "m/d/yyyy", "0", "1", serial(2024, 1, 15, system=1904), "-1", "2957003", "2957004", "-1462", system=1904)
add("d1904", "dddd", "0", system=1904)
add("d1904", "yyyy-mm-dd hh:mm:ss", serial(2024, 1, 15, 14, 30, 45, system=1904), system=1904)
add("d1904", "h:mm", "-0.5", system=1904)
add("d1904", "[h]:mm", "-1.5", system=1904)
add("d1904", "[h]:mm:ss", "1.5", system=1904)
add("d1904", "General", "5", system=1904)
add("d1904", "builtin:14", "0", system=1904)

# Round 2: what round 1 left open.
ROUND = 2
add("r2cond", "[<100]0;0", "-5")
add("r2cond", "[>-10]0;0", "-5")
add("r2cond", "[<0]0", "-5")
add("r2cond", "[<100]0", "-5")
add("r2cond", "[<=0]0;0", "-5")
add("r2cond", "[=-5]0;0", "-5")
add("r2cond", "[<>5]0;0", "-5")
add("r2cond", "[<-10]0;[<0]0;0", "-5")
add("r2cond", "[<-10]0;[>0]0;0", "-5")
add("r2cond", "[<100]\"a\";\"b\"", "-5")
add("r2cond", "[<0]\"neg\";0", "-5")
add("r2cond", "[>=100]0;[<0]0", "-5", "50")
add("r2gen", "General;-General", "-1.5")
add("r2gen", "General;General", "-1.5")
add("r2gen", "\"Total: \"General", "1234.5", "-1234.5")
add("r2gen", "0.00;General", "-1234.567")
add("r2gen", "[Red]General", "-0.123456789012")
add("r2num", ".00", "12.5", "0.5")
add("r2num", "#.00", "0.5")
add("r2num", "0", "1E+300", "-1E+300")
add("r2num", "#,##0", "-0.4")
add("r2num", "\"x\"", "-0.4", "-5", "0")
add("r2num", "0.0", "-0.04")
add("r2frac", "?/?", "0", "-0.5")
add("r2frac", "# ?/?", "-0.01", "0.04", "-1.0001")
add("r2frac", "# ??/??", "0.999")
add("r2exp", "0.00E+00", "-0.000000001")
add("r2time", "m/d/yyyy", "45306.9997", "45306.99999")
add("r2time", "h:mm", "0.9997")
add("r2time", "h:mm:ss", "0.99999")
add("r2time", "[$-F400]h:mm:ss AM/PM", "0.25")
for n in [60, 61, 63, 64, 65, 66, 68, 70, 72, 73, 74, 75, 77, 78, 80]:
    add("r2builtin", f"builtin:{n}", DT, "-1234.567")


def check_values():
    for case_id, _code, value, _system, kind, _round in CASES:
        if kind != "number":
            continue
        written = f"{float(value):.16G}"
        if float(written) != float(value):
            sys.exit(f"{case_id}: {value} does not survive %.16G ({written})")


def write_tsv(path, cases):
    lines = ["id\tcode\tvalue\tdate_system\tkind"]
    for case in cases:
        assert all("\t" not in field and "\n" not in field for field in case), case
        lines.append("\t".join(case))
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def write_workbook(path, system, cases):
    workbook = xlsxwriter.Workbook(str(path), {"date_1904": system == 1904, "strings_to_numbers": False})
    sheet = workbook.add_worksheet("cases")
    sheet.set_column(0, 0, 12)
    sheet.set_column(1, 1, 40)
    sheet.set_column(2, 2, 40)
    sheet.set_column(3, 3, 8)
    sheet.write_row(0, 0, ["id", "code", "displayed", "kind"])
    formats = {}
    for row, (case_id, code, value, _system, kind) in enumerate(cases, start=1):
        if code not in formats:
            num_format = int(code.split(":")[1]) if code.startswith("builtin:") else code
            formats[code] = workbook.add_format({"num_format": num_format})
        cell_format = formats[code]
        sheet.write_string(row, 0, case_id)
        sheet.write_string(row, 1, code)
        if kind == "number":
            sheet.write_number(row, 2, float(value), cell_format)
        elif kind == "text":
            sheet.write_string(row, 2, value, cell_format)
        else:
            sheet.write_boolean(row, 2, value == "TRUE", cell_format)
        sheet.write_string(row, 3, kind)
    workbook.close()


def main(out, number):
    check_values()
    ids = [case[0] for case in CASES]
    assert len(ids) == len(set(ids)), "case ids are unique"
    cases = [case[:5] for case in CASES if case[5] == number]
    if not cases:
        sys.exit(f"no case in round {number}")
    out.mkdir(parents=True, exist_ok=True)
    write_tsv(out / f"cases-r{number}.tsv", cases)
    for system in ("1900", "1904"):
        of_system = [case for case in cases if case[3] == system]
        if of_system:
            write_workbook(out / f"cases-r{number}-{system}.xlsx", int(system), of_system)
    print(f"round {number}: {len(cases)} cases in {out}")


if __name__ == "__main__":
    main(pathlib.Path(sys.argv[1]), int(sys.argv[2]))
