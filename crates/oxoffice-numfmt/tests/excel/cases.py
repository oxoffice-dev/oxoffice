"""The cases of tests/cases.tsv, grouped by the Excel round that checked them.

Writes the cases of one round into an output directory:
- cases-rN.tsv: id, code, value, date_system, kind (one row per case);
- cases-rN-1900.xlsx and cases-rN-1904.xlsx: the cases of each date system (a workbook setting), sheet
  "cases": A id, B code (text), C the value written with the code applied (the cell Excel reads), D the
  kind. Column C is 40 characters wide. Excel keeps the first 215 custom number formats of a workbook and
  reads the others as General: a workbook holds at most MAX_FORMATS codes, and a round with more is written in
  parts, cases-rN-1900-p2.xlsx and so on (pin.py takes the CSV of every part).

Conventions:
- code `builtin:N` is the built-in format id N, written as numFmtId N with no numFmt record;
- kind `number` (value is a decimal literal) or `text` (value is the string);
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

# Round 3: a condition on the second section only, and codes or values at the limits.
ROUND = 3
add("r3cond", "0;[<0]\"neg\"", "5", "-5", "0")
add("r3cond", "0;[<0]\"neg\";\"zero\"", "5", "-5", "0")
add("r3cond", "0;[>100]\"big\"", "5", "200", "-5")
add("r3cond", "0;[>100]\"big\";\"zero\"", "0", "5", "-5", "200")
add("r3cond", "\"pos\";[<0]\"neg\"", "5", "-5")
add("r3cond", "0.0;[=0]\"zero\"", "0", "5", "-5")
add("r3limit", "[" + "h" * 20 + "]:mm", "1.5")
add("r3limit", "[" + "h" * 100 + "]", "1.5")
add("r3limit", "0\"" + "x" * 297 + "\"", "5")
add("r3limit", "?/100", "1E+37")
add("r3limit", "# ?/16", "1E-39")
add("r3limit", "?/16", "1E-39")

# Round 4: month and day names and the day period under a language tag. Each month case writes the three month
# tokens of one month, each day case the two day tokens of one weekday, each period case the four spellings of
# the day period for a morning and an afternoon, `|` apart: the whole table of a language in 12 + 7 + 2 cases.
ROUND = 4
MONTH_CODE = 'mmm"|"mmmm"|"mmmmm'
DAY_CODE = 'ddd"|"dddd'
PERIOD_CODE = 'h AM/PM"|"am/pm"|"A/P"|"a/p'
TAGGED_LOCALES = [  # (LCID of the tag, what it is)
    # English, French, German, Spanish, Italian, Portuguese and Dutch: the languages of Word's date controls.
    ("409", "en-US"), ("809", "en-GB"), ("C09", "en-AU"), ("1009", "en-CA"), ("4009", "en-IN"),
    ("40C", "fr-FR"), ("80C", "fr-BE"), ("C0C", "fr-CA"), ("100C", "fr-CH"),
    ("407", "de-DE"), ("C07", "de-AT"), ("807", "de-CH"),
    ("C0A", "es-ES"), ("80A", "es-MX"), ("2C0A", "es-AR"),
    ("410", "it-IT"), ("810", "it-CH"),
    ("416", "pt-BR"), ("816", "pt-PT"),
    ("413", "nl-NL"), ("813", "nl-BE"),
]
for lcid, _name in TAGGED_LOCALES:
    for month in range(1, 13):
        add(f"r4m-{lcid}", f"[$-{lcid}]{MONTH_CODE}", serial(2024, month, 15))
    for day in range(14, 21):  # Sunday 2024-01-14 to Saturday 2024-01-20
        add(f"r4d-{lcid}", f"[$-{lcid}]{DAY_CODE}", serial(2024, 1, day))
    add(f"r4p-{lcid}", f"[$-{lcid}]{PERIOD_CODE}", serial(2024, 1, 15, 5, 9, 0), PM5)
# The tag as text, as newer Excel writes it: a language and region, or a language.
TEXT_TAGS = ["fr-FR", "fr", "fr-fr", "fr-CA", "de-DE", "de-AT", "de", "es-ES", "es-MX", "es", "it-IT", "pt-BR",
             "pt-PT", "nl-NL", "nl", "en-GB", "en-US", "en"]
for tag in TEXT_TAGS:
    add("r4t", f"[$-{tag}]ddd\"|\"dddd\"|\"mmm\"|\"mmmm\"|\"mmmmm", serial(2024, 9, 4))
# Languages no table of ours holds: what Excel shows for them.
for lcid in ["411", "804", "412", "419", "415", "41D", "41F", "403", "401"]:
    add("r4o", f"[$-{lcid}]ddd\"|\"dddd\"|\"mmm\"|\"mmmm\"|\"mmmmm", serial(2024, 9, 4))
    add("r4o", f"[$-{lcid}]{PERIOD_CODE}", PM5)
# Where the tag stands and which one counts: the section's, wherever it is; the sections have their own.
add("r4s", "dddd d mmmm[$-40C]", D)
add("r4s", "[$-40C][$-407]mmmm", D)
add("r4s", "[$-407][$-40C]mmmm", D)
add("r4s", '[>40000][$-407]mmmm;[$-40C]mmmm', "45306", "30000")
add("r4s", '[$-40C]mmmm;[$-407]mmmm', "45306")
add("r4s", '[$€-40C]mmmm', D)

# Round 5: what round 4 left open. Its period cases wrote four spellings of the day period in one code and Excel
# showed the full designator for all of them (`A/P` too), so here each spelling has its own code, with and without
# a tag; the regions of Spanish (a language with four spellings of its short names), French, German and English
# not seen; the tag without a region; the high word of an LCID; and which sections a tag speaks for.
ROUND = 5
MORNING = serial(2024, 1, 15, 5, 9, 0)
for lcid in ["409", "809", "C0C", "C0A", "80A"]:
    for spelling in ["AM/PM", "am/pm", "A/P", "a/p"]:
        add(f"r5p-{lcid}", f"[$-{lcid}]h {spelling}", MORNING, PM5)
for spelling in ["AM/PM", "am/pm", "A/P", "a/p"]:
    add("r5p-none", f"h {spelling}", MORNING, PM5)
add("r5p-none", PERIOD_CODE, MORNING, PM5)
add("r5p-none", 'h A/P"|"AM/PM', MORNING, PM5)
add("r5p-none", 'h AM/PM"|"A/P', MORNING, PM5)
for lcid in ["1409", "1809", "1C09"]:  # en-NZ, en-IE, en-ZA: the day period only (English names are the same)
    add(f"r5p-{lcid}", f"[$-{lcid}]h AM/PM", MORNING, PM5)
MORE_LOCALES = ["40A", "240A", "340A", "540A", "140C", "1007"]  # es-ES (traditional), es-CO, es-CL, es-US, fr-LU, de-LU
for lcid in MORE_LOCALES:
    for month in range(1, 13):
        add(f"r5m-{lcid}", f"[$-{lcid}]{MONTH_CODE}", serial(2024, month, 15))
    for day in range(14, 21):
        add(f"r5d-{lcid}", f"[$-{lcid}]{DAY_CODE}", serial(2024, 1, day))
    add(f"r5p-{lcid}", f"[$-{lcid}]h AM/PM", MORNING, PM5)
for tag in ["es", "pt"]:  # a language with no region
    for month in range(1, 13):
        add(f"r5m-{tag}", f"[$-{tag}]{MONTH_CODE}", serial(2024, month, 15))
    for day in range(14, 21):
        add(f"r5d-{tag}", f"[$-{tag}]{DAY_CODE}", serial(2024, 1, day))
    add(f"r5p-{tag}", f"[$-{tag}]h AM/PM", MORNING, PM5)
# The high word of an LCID (calendar and digits), the low word names the language.
for lcid in ["1040C", "2040C", "10407", "F40C"]:
    add("r5h", f"[$-{lcid}]{DAY_CODE}\"|\"{MONTH_CODE}", serial(2024, 9, 4))
# Which sections a tag speaks for: the tag of the first section, of the second only, of each.
add("r5s", "[>40000]mmmm;[$-40C]mmmm", "45306", "30000")
add("r5s", "[>40000][$-40C]mmmm;mmmm", "45306", "30000")
add("r5s", "[$-407]mmmm;[$-40C]mmmm;mmmm", "45306")
add("r5s", "[<40000][$-407]mmmm;[>=40000][$-40C]mmmm", "45306", "30000")
add("r5s", '[$-40C]d mmmm yyyy;@', D)
add("r5s", '[$-40C]d mmmm yyyy;@', "abc", system=1900, kind="text")


# Round 6: the names of the languages round 4 saw with no table (#707): Japanese, Chinese, Korean, Russian, Polish,
# Swedish, Turkish, Catalan and Arabic. For each region of each language, the 12 months (`mmm`, `mmmm`, `mmmmm`), the
# 7 weekdays (`ddd`, `dddd`) and the day period of a morning and an afternoon; for the first region of each language
# the other spellings of the day period and the day period at midnight and noon; then what may change a month's form
# (Russian and Polish use a genitive month after a day number in their system data), full date formats as a user
# picks them, and the tag as text or as a neutral LCID. The ids carry the table: `r6m-<LCID>-NN` is month NN,
# `r6d-<LCID>-NN` weekday NN (Sunday first), `r6p-<LCID>-01` and `-02` the morning and the afternoon.
ROUND = 6
EVENING = PM5  # 2024-01-15 17:05:09
SMALL_HOURS = serial(2024, 1, 15, 0, 30, 0)
AFTER_NOON = serial(2024, 1, 15, 12, 30, 0)
SEPTEMBER = serial(2024, 9, 4, 14, 30, 45)  # a Wednesday
LANGUAGES = [  # (language, the first region asked, the other regions asked)
    ("ja", "411", []),
    ("zh", "804", ["404", "C04", "1004", "1404"]),
    ("ko", "412", []),
    ("ru", "419", ["819"]),
    ("pl", "415", []),
    ("sv", "41D", ["81D"]),
    ("tr", "41F", []),
    ("ca", "403", ["803"]),
    ("ar", "401", ["801", "C01", "1001", "1401", "1801", "1C01", "2001", "2401", "2801", "2C01", "3001", "3401",
                   "3801", "3C01", "4001"]),
]
for _language, first, others in LANGUAGES:
    for lcid in [first, *others]:
        for month in range(1, 13):
            add(f"r6m-{lcid}", f"[$-{lcid}]{MONTH_CODE}", serial(2024, month, 15))
        for day in range(14, 21):
            add(f"r6d-{lcid}", f"[$-{lcid}]{DAY_CODE}", serial(2024, 1, day))
        add(f"r6p-{lcid}", f"[$-{lcid}]h AM/PM", MORNING, EVENING)
    for spelling in ["am/pm", "A/P", "a/p"]:
        add(f"r6p-{first}", f"[$-{first}]h {spelling}", MORNING, EVENING)
    add(f"r6p-{first}", f"[$-{first}]h AM/PM", SMALL_HOURS, AFTER_NOON)

# Does a month take another form next to a day number? Russian and Polish: every month, five codes, then where the
# day number may stand and what else may count (three months: January, March and May, whose short forms differ in
# Russian).
FIVE = ["d mmmm", "d mmm", "dd mmmm yyyy", "mmmm yyyy", "mmmm d"]
WHERE = ["dddd mmmm", "ddd d mmm", "mmmm yyyy d", "m/d/yyyy mmmm", 'd"."mmmm', 'd" x "mmmm', 'mmmm" x "d', "d-mmm-yy",
         "d mmmmm", "dddd, d mmmm yyyy"]
for lcid in ["419", "415"]:
    for code in FIVE:
        for month in range(1, 13):
            add(f"r6c-{lcid}", f"[$-{lcid}]{code}", serial(2024, month, 15))
    for code in WHERE:
        add(f"r6c-{lcid}", f"[$-{lcid}]{code}", *[serial(2024, month, 15) for month in (1, 3, 5)])
# The other languages: the two codes with a day number for every month, and three more for January and September.
for lcid in ["411", "804", "404", "412", "41D", "41F", "403", "401", "801"]:
    for code in ["d mmmm", "d mmm"]:
        for month in range(1, 13):
            add(f"r6c-{lcid}", f"[$-{lcid}]{code}", serial(2024, month, 15))
    for code in ["dd mmmm yyyy", "mmmm yyyy", "mmmm d"]:
        add(f"r6c-{lcid}", f"[$-{lcid}]{code}", serial(2024, 1, 15), serial(2024, 9, 15))

# Full date formats as a user picks them, on one date (a Wednesday afternoon), the first two on a January one too.
FULL = ["dddd, mmmm d, yyyy", "dddd, d mmmm yyyy", "d mmmm yyyy", "d-mmm-yy", "mmm d, yyyy", "ddd, d mmm yyyy",
        "mmmm yyyy", "dddd, mmmm d, yyyy h:mm AM/PM", "d mmmm yyyy h:mm:ss"]
LANGUAGE_FULL = {
    "411": ['yyyy"年"m"月"d"日" dddd', 'm"月"d"日" ddd'],
    "804": ['yyyy"年"m"月"d"日" dddd', 'm"月"d"日" ddd'],
    "404": ['yyyy"年"m"月"d"日" dddd'],
    "412": ['yyyy"년" m"월" d"일" dddd'],
    "419": ['d mmmm yyyy "г."', 'dddd, d mmmm yyyy "г."'],
    "415": ['d mmmm yyyy "r."', 'dddd, d mmmm yyyy "r."'],
    "41F": ["d mmmm yyyy dddd"],
}
for lcid in ["411", "804", "404", "412", "419", "415", "41D", "41F", "403", "401"]:
    for code in FULL + LANGUAGE_FULL.get(lcid, []):
        add(f"r6f-{lcid}", f"[$-{lcid}]{code}", SEPTEMBER)
    if lcid in ("419", "415"):
        for code in FULL[:3]:
            add(f"r6f-{lcid}", f"[$-{lcid}]{code}", serial(2024, 1, 15, 14, 30, 45))

# The tag as text, as newer Excel writes it, and as a neutral LCID (a language with no region): one case each, the
# names of a Wednesday of September and the day period of an afternoon.
TAG_CODE = 'ddd"|"dddd"|"mmm"|"mmmm"|"mmmmm"|"h AM/PM'
for tag in ["ja", "ja-JP", "ja-jp", "zh", "zh-CN", "zh-Hans", "zh-TW", "zh-Hant", "zh-HK", "zh-SG", "zh-MO", "ko",
            "ko-KR", "ru", "ru-RU", "ru-ru", "ru-MD", "pl", "pl-PL", "sv", "sv-SE", "sv-FI", "tr", "tr-TR", "ca",
            "ca-ES", "ca-ES-valencia", "ar", "ar-SA", "ar-EG", "ar-JO", "ar-IQ", "ar-MA", "ar-DZ", "ar-AE", "ar-TN"]:
    add("r6t", f"[$-{tag}]{TAG_CODE}", SEPTEMBER)
for lcid in ["1", "3", "4", "7C04", "11", "12", "15", "19", "1D", "1F"]:  # the neutral LCIDs of these languages
    add("r6n", f"[$-{lcid}]{TAG_CODE}", SEPTEMBER)

# Round 7: what round 6 may not read as a name: `aaa` and `aaaa`, the weekday of East Asian Excel (`[$-804]aaaa` is
# Chinese Excel's own weekday format), and the eras (`g`, `gg`, `ggg`, `e`) of the Japanese, Taiwanese and Korean
# calendars. Separate from round 6: a code Excel cannot read may repair the file on open.
ROUND = 7
for tag in ["", "409", "411", "804", "404", "412", "419", "401"]:
    prefix = f"[$-{tag}]" if tag else ""
    add("r7a", f'{prefix}aaa"|"aaaa', SEPTEMBER)
for lcid in ["411", "804", "404", "412"]:
    for day in range(14, 21):
        add(f"r7w-{lcid}", f'[$-{lcid}]aaa"|"aaaa', serial(2024, 1, day))
for code in ['[$-411]ggge"年"m"月"d"日"', '[$-411]g"|"gg"|"ggg', '[$-411]e', '[$-404]ggge"年"m"月"d"日"',
             '[$-404]e', '[$-412]e', '[$-804]e', 'e']:
    add("r7e", code, SEPTEMBER, serial(2019, 5, 1), serial(1989, 1, 7))


# Round 8: what round 6 left open: the Chinese tags with no region, as Excel rewrites `zh` and `zh-Hans` (`7804` and
# `4`), and `zh-Hant` (`7C04`): their twelve months, seven weekdays and day period.
ROUND = 8
for lcid in ["4", "7804", "7C04"]:
    for month in range(1, 13):
        add(f"r8m-{lcid}", f"[$-{lcid}]{MONTH_CODE}", serial(2024, month, 15))
    for day in range(14, 21):
        add(f"r8d-{lcid}", f"[$-{lcid}]{DAY_CODE}", serial(2024, 1, day))
    add(f"r8p-{lcid}", f"[$-{lcid}]h AM/PM", MORNING, EVENING)


# Round 9: the Japanese eras under `[$-411]` (round 7 pinned one date of each of three eras): each era's first day and
# the day before, the era year as `e` and `ee`, the era tokens with times, under the other forms of the tag, under a
# tag with no era, and the dates the 1900 system holds before Meiji 45 (serials 1, 59, 60, 61). Meiji began in 1868,
# before the first date a workbook holds, so its first day cannot be asked.
ROUND = 9
ERA_DATE = 'ggge"年"m"月"d"日"'
ERA_STARTS = {  # era: (first day, the day before)
    "taisho": (serial(1912, 7, 30), serial(1912, 7, 29)),
    "showa": (serial(1926, 12, 25), serial(1926, 12, 24)),
    "heisei": (serial(1989, 1, 8), serial(1989, 1, 7)),
    "reiwa": (serial(2019, 5, 1), serial(2019, 4, 30)),
}
BOUNDARIES = [day for pair in ERA_STARTS.values() for day in pair]
for era, days in ERA_STARTS.items():
    add(f"r9a-{era}", f"[$-411]{ERA_DATE}", *days)
    add(f"r9g-{era}", '[$-411]g"|"gg"|"ggg', *days)
    add(f"r9e-{era}", '[$-411]e"|"ee', *days)
# The era years: the first, the tenth, the last of Showa, a two-digit year of Heisei, the first Reiwa year as Gannen.
YEARS = [serial(1900, 3, 1), serial(1912, 7, 29), serial(1935, 6, 15), serial(1964, 10, 10), serial(1989, 1, 7),
         serial(1998, 2, 1), serial(2019, 4, 30), serial(2019, 12, 31), serial(2020, 1, 1), serial(2025, 12, 31)]
add("r9y", f"[$-411]{ERA_DATE}", *YEARS)
add("r9y", '[$-411]e"|"ee', *YEARS)
add("r9y", '[$-411]ggge/m/d', *YEARS[:5])
add("r9y", '[$-411]gge.mm.dd', *YEARS[:5])
add("r9y", '[$-411]yyyy"|"yy"|"e"|"ggg', *YEARS[2:5])
# The dates of the 1900 system before the first real boundary, and the dates the system gets wrong.
add("r9s", f"[$-411]{ERA_DATE}", 1, 59, 60, 61, 0.5)
add("r9s", '[$-411]g"|"gg"|"ggg"|"e"|"ee', 1, 60)
# Times: the era with a time of day, a time that rounds across a boundary, and a time alone.
add("r9t", f'[$-411]{ERA_DATE} h:mm:ss', *[days[1] + 0.75 for days in ERA_STARTS.values()], serial(2019, 5, 1, 0, 0, 1))
add("r9t", f'[$-411]{ERA_DATE} h:mm AM/PM', serial(2019, 5, 1, 9, 5), serial(2019, 4, 30, 21, 5))
add("r9t", f'[$-411]{ERA_DATE} h:mm:ss', 43585.9999999, 43585.99999)
add("r9t", f'[$-411]{ERA_DATE}', 43585.9999999, 43585.99999)
add("r9t", '[$-411]ggge"年"m"月"d"日" hh:mm', serial(2024, 9, 4, 14, 30))
add("r9t", '[$-411]h:mm:ss', 0.5)
# The other forms of the tag: a neutral LCID, a zero-padded one, and the text tags.
for tag in ["11", "0411", "ja", "ja-JP", "ja-jp", "JA-JP"]:
    add(f"r9f-{tag}", f"[$-{tag}]{ERA_DATE}", SEPTEMBER, serial(2019, 5, 1), serial(1989, 1, 7))
    add(f"r9f-{tag}", f'[$-{tag}]g"|"gg"|"ggg"|"e"|"ee', serial(2019, 5, 1), serial(1989, 1, 7))
# A tag that writes the Gregorian year, none, and the codes where Excel shows nothing for the era tokens.
for tag in ["409", "404", "412", "804", "40C", "41F", ""]:
    prefix = f"[$-{tag}]" if tag else ""
    add(f"r9n-{tag or 'none'}", f'{prefix}g"|"gg"|"ggg"|"e"|"ee', SEPTEMBER, serial(2019, 5, 1), serial(1989, 1, 7))
add("r9n-none", ERA_DATE, SEPTEMBER, serial(1989, 1, 7))
add("r9n-409", f"[$-409]{ERA_DATE}", SEPTEMBER, serial(1989, 1, 7))
add("r9n-none", 'ggg"|"gg"|"g', SEPTEMBER)
# The 1904 system: the same dates.
add("r9z", f"[$-411]{ERA_DATE}", serial(2019, 5, 1, system=1904), serial(2019, 4, 30, system=1904),
    serial(1989, 1, 8, system=1904), system=1904)
add("r9z", '[$-411]e"|"ee', serial(2019, 5, 1, system=1904), serial(1989, 1, 7, system=1904), system=1904)


# Round 10: a tag with a script and a region (what BCP 47 producers write): which of the two Excel reads, for Chinese.
ROUND = 10
for tag in ["zh-Hans-CN", "zh-Hans-SG", "zh-Hans-HK", "zh-Hans-TW", "zh-Hant-HK", "zh-Hant-TW", "zh-Hant-MO",
            "zh-Hant-CN", "zh-Hant-SG"]:
    add("r10t", f"[$-{tag}]{TAG_CODE}", SEPTEMBER)


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
        else:
            sheet.write_string(row, 2, value, cell_format)
        sheet.write_string(row, 3, kind)
    workbook.close()


MAX_FORMATS = 200  # custom number formats of one workbook: Excel keeps the first 215 and reads the rest as General


def parts(cases):
    """The cases in order, cut into workbooks of at most MAX_FORMATS distinct codes."""
    chunks, codes = [[]], set()
    for case in cases:
        code = case[1]
        custom = not code.startswith("builtin:") and code != "General"
        if custom and code not in codes and len(codes) == MAX_FORMATS:
            chunks.append([])
            codes = set()
        if custom:
            codes.add(code)
        chunks[-1].append(case)
    return chunks


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
        for index, part in enumerate(parts(of_system) if of_system else [], start=1):
            suffix = "" if index == 1 else f"-p{index}"
            write_workbook(out / f"cases-r{number}-{system}{suffix}.xlsx", int(system), part)
    print(f"round {number}: {len(cases)} cases in {out}")


if __name__ == "__main__":
    main(pathlib.Path(sys.argv[1]), int(sys.argv[2]))

