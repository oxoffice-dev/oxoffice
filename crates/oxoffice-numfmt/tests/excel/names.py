"""Writes src/names/tables.rs, the month and day names of every language, from the Excel rows of tests/cases.tsv.

A language is one Excel round (cases.py: its tags with `mmm mmmm mmmmm`, `ddd dddd` and `h AM/PM`), pinned (pin.py)
and a few lines of LANGUAGES below: which Rust table each pinned tag is, and which region tags and LCIDs read as
which table. The tables hold what Excel for Mac showed, nothing else: a tag whose rows differ from its table's, a
spelling of the day period that shows another designator, or a pinned tag no table claims stops the run.

Run from this directory: python3 -I names.py   (writes ../../src/names/tables.rs; then cargo fmt is a no-op)
"""

import pathlib
import re
import sys
import unicodedata

HERE = pathlib.Path(__file__).resolve().parent
CASES = HERE.parent / "cases.tsv"
OUT = HERE.parent.parent / "src" / "names" / "tables.rs"

MORNING = "45306.2145833333"  # 2024-01-15 05:09:00
AFTERNOON = "45306.7119097222"  # 2024-01-15 17:05:09


class Table:
    """One table of names.

    `pinned`: the tags (as written in `[$-tag]`: an LCID, or `es` for a language with no region) whose month, weekday
    and day period rows are this table's; a tag as text (`fr-FR`) is checked by its own rows of the case table. `regions`: `(LCID, tag)` pairs, the
    regions that read as this table where the language's default is another (an LCID is its low word in hex, a tag
    is `language` or `language-region` in lower case, empty for an LCID with no text tag; LCID 0 for a tag with no LCID, as `zh-hans-cn`); a third item is a note. `public`: Word's date controls (oxoffice-docx-oxml) use it.
    """

    def __init__(self, name, doc, pinned, regions=(), public=False):
        self.name, self.doc, self.pinned, self.regions, self.public = name, doc, pinned, list(regions), public


class Language:
    """A language: its primary language id (the low ten bits of an LCID), its tag, its default table."""

    def __init__(self, primary, tag, tables):
        self.primary, self.tag, self.tables = primary, tag, tables

    @property
    def default(self):
        return self.tables[0]


LANGUAGES = [
    Language(0x09, "en", [
        Table("ENGLISH", "United States English, also Canada, India, New Zealand and South Africa.",
              ["409", "1009", "4009"],
              [(0x0001, "", "the neutral Arabic LCID `[$-1]` shows English names in Excel, `[$-ar]` Arabic ones")],
              public=True),
        Table("ENGLISH_UK", "British English, also Australia and Ireland: the day period in lower case.",
              ["809", "C09"], [(0x0809, "en-gb"), (0x0C09, "en-au"), (0x1809, "en-ie")]),
    ]),
    Language(0x0C, "fr", [
        Table("FRENCH", "French of France, also Belgium, Switzerland and Luxembourg.",
              ["40C", "80C", "100C", "140C"], public=True),
        Table("FRENCH_CANADA", "Canadian French: `juill.` and `a.m.`.", ["C0C"], [(0x0C0C, "fr-ca")]),
    ]),
    Language(0x07, "de", [
        Table("GERMAN", "German of Germany, also Switzerland and Luxembourg.",
              ["407", "807", "1007"], public=True),
        Table("GERMAN_AUSTRIA", "Austrian German names January `Jänner`.", ["C07"],
              [(0x0C07, "de-at")], public=True),
    ]),
    Language(0x0A, "es", [
        Table("SPANISH", "Spanish of Spain, the table of a region of Spanish not asked for.",
              ["C0A", "40A"], public=True),
        Table("SPANISH_MEXICO", "Mexican Spanish.", ["80A"], [(0x080A, "es-mx")]),
        Table("SPANISH_ARGENTINA", "Argentine Spanish.", ["2C0A"], [(0x2C0A, "es-ar")]),
        Table("SPANISH_COLOMBIA", "Colombian Spanish.", ["240A"], [(0x240A, "es-co")]),
        Table("SPANISH_CHILE", "Chilean Spanish.", ["340A"], [(0x340A, "es-cl")]),
        Table("SPANISH_US", "Spanish of the United States.", ["540A"], [(0x540A, "es-us")]),
        Table("SPANISH_NO_REGION", "The tag `es`, with no region.", ["es"], [(0x000A, "es")]),
    ]),
    Language(0x10, "it", [
        Table("ITALIAN", "Italian, also Switzerland.", ["410", "810"], public=True),
    ]),
    Language(0x16, "pt", [
        Table("PORTUGUESE", "Portuguese of Brazil, the table of a region of Portuguese not asked for.",
              ["416"], public=True),
        Table("PORTUGUESE_PORTUGAL", "Portuguese of Portugal.", ["816"], [(0x0816, "pt-pt")]),
        Table("PORTUGUESE_NO_REGION", "The tag `pt`, with no region.", ["pt"], [(0x0016, "pt")]),
    ]),
    Language(0x13, "nl", [
        Table("DUTCH", "Dutch, also Belgium.", ["413", "813"], public=True),
    ]),
    Language(0x11, "ja", [Table("JAPANESE", "Japanese: `1月`, `水曜日`, `午前`.", ["411"])]),
    Language(0x04, "zh", [
        Table("CHINESE_SIMPLIFIED", "Chinese of China: `一月`, `1月`, `周三`.", ["804"], [(0x0804, "zh-cn"), (0, "zh-hans-cn"), (0, "zh-hans-hk")]),
        Table("CHINESE_TAIWAN", "Chinese of Taiwan and Macau (and `zh-Hant`): `9月` whole, `週三`.", ["404", "1404", "7C04"],
              [(0x0404, "zh-tw"), (0x1404, "zh-mo"), (0x7C04, "zh-hant"), (0, "zh-hant-tw"), (0, "zh-hant-mo"),
               (0, "zh-hant-cn"), (0, "zh-hant-sg")]),
        Table("CHINESE_NO_REGION", "The tags `zh` and `zh-Hans` (`[$-7804]`, `[$-4]`): Simplified names, `週三`.",
              ["4", "7804"], [(0x7804, "zh"), (0x0004, "zh-hans"), (0, "zh-hans-tw")]),
        Table("CHINESE_HONG_KONG", "Chinese of Hong Kong: `九月` whole, `週三`.", ["C04"], [(0x0C04, "zh-hk"), (0, "zh-hant-hk")]),
        Table("CHINESE_SINGAPORE", "Chinese of Singapore: `九月` whole, `周三`.", ["1004"], [(0x1004, "zh-sg"), (0, "zh-hans-sg")]),
    ]),
    Language(0x12, "ko", [Table("KOREAN", "Korean: `9월`, `수요일`, `오후`.", ["412"])]),
    Language(0x19, "ru", [
        Table("RUSSIAN", "Russian: the day period in Latin letters (`AM`, `PM`), the months as Excel writes them "
              "after a day number too (`15 январь`).", ["419", "819"]),
    ]),
    Language(0x15, "pl", [
        Table("POLISH", "Polish: the day period in Latin letters (`AM`, `PM`), the months as Excel writes them "
              "after a day number too (`15 styczeń`).", ["415"]),
    ]),
    Language(0x1D, "sv", [Table("SWEDISH", "Swedish, also Finland: `fm` and `em`.", ["41D", "81D"])]),
    Language(0x1F, "tr", [Table("TURKISH", "Turkish: `ÖÖ` and `ÖS`.", ["41F"])]),
    Language(0x03, "ca", [Table("CATALAN", "Catalan, also its Valencian variant.", ["403", "803"])]),
    Language(0x01, "ar", [
        Table("ARABIC", "Arabic of Saudi Arabia and the regions that write `يناير`; short names are the full ones.",
              ["401", "C01", "1001", "1401", "1801", "1C01", "2001", "2401", "3401", "3801", "3C01", "4001"],
              [(0x0C01, "ar-eg"), (0x1001, "ar-ly"), (0x1401, "ar-dz"), (0x1801, "ar-ma"), (0x1C01, "ar-tn"),
               (0x2001, "ar-om"), (0x2401, "ar-ye"), (0x3401, "ar-kw"), (0x3801, "ar-ae"), (0x3C01, "ar-bh"),
               (0x4001, "ar-qa")]),
        Table("ARABIC_LEVANT", "Arabic of Iraq, Syria, Jordan and Lebanon: `كانون الثاني`, `شباط`.",
              ["801", "2801", "2C01", "3001"],
              [(0x0801, "ar-iq"), (0x2801, "ar-sy"), (0x2C01, "ar-jo"), (0x3001, "ar-lb")]),
    ]),
]

MONTHS = re.compile(r'^\[\$-([0-9A-Za-z-]+)\]mmm"\|"mmmm"\|"mmmmm$')
DAYS = re.compile(r'^\[\$-([0-9A-Za-z-]+)\]ddd"\|"dddd$')


def rows():
    """code -> [(value, excel)] of every row of the table."""
    by_code = {}
    for line in CASES.read_text(encoding="utf-8").splitlines()[1:]:
        _id, code, value, _system, _kind, excel, _ours, _why = line.split("\t")
        by_code.setdefault(code, []).append((value, excel))
    return by_code


def names_of(by_code, tag):
    """The five arrays one tag's rows give, or None when it has no month rows."""
    months = by_code.get(f'[$-{tag}]mmm"|"mmmm"|"mmmmm')
    days = by_code.get(f'[$-{tag}]ddd"|"dddd')
    if months is None and days is None:
        return None
    assert months and days and len(months) == 12 and len(days) == 7, f"{tag}: 12 months and 7 weekdays"
    short_months, full_months, short_days, full_days = [], [], [], []
    for _value, excel in sorted(months, key=lambda row: float(row[0])):
        short, full, letter = excel.split("|")
        assert letter == full[0], f"{tag}: the one-letter month of {full} is {letter}, not its first letter"
        short_months.append(short)
        full_months.append(full)
    for _value, excel in sorted(days, key=lambda row: float(row[0])):
        short, full = excel.split("|")
        short_days.append(short)
        full_days.append(full)
    return {
        "months": full_months,
        "short_months": short_months,
        "days": full_days,
        "short_days": short_days,
        "day_periods": day_periods(by_code, tag),
    }


def day_periods(by_code, tag):
    """[morning, afternoon]: from `h AM/PM` alone, or the first of the four spellings round 4 asked at once."""
    periods = []
    for value in (MORNING, AFTERNOON):
        alone = dict(by_code.get(f"[$-{tag}]h AM/PM", []))
        together = dict(by_code.get(f'[$-{tag}]h AM/PM"|"am/pm"|"A/P"|"a/p', []))
        shown = alone.get(value) or together.get(value, "").split("|")[0]
        assert shown.startswith("5 "), f"{tag}: no day period rows"
        periods.append(shown[2:])
    # Every other spelling shows the same designator, as the Latin languages do; a language where one does not
    # needs a field for it.
    for spelling in ("am/pm", "A/P", "a/p"):
        for value, designator in zip((MORNING, AFTERNOON), periods):
            shown = dict(by_code.get(f"[$-{tag}]h {spelling}", [])).get(value)
            assert shown in (None, f"5 {designator}"), f"{tag}: {spelling} shows {shown!r}, not {designator!r}"
    return periods


def check_regions(by_code, table_names):
    """Every region of LANGUAGES has rows, is in one table only, and reads as the table its rows show."""
    # The tags the case table asked for, by lower case: as written.
    asked = {}
    for code in by_code:
        tag = re.match(r"^\[\$-([0-9A-Za-z-]+)\]", code)
        if tag:
            asked.setdefault(tag.group(1).lower(), tag.group(1))
    seen_pinned, seen_lcids, seen_tags = {}, {}, {}
    for language in LANGUAGES:
        for table in language.tables:
            for tag in table.pinned:
                assert tag.lower() not in seen_pinned, f"{tag} is pinned for {seen_pinned[tag.lower()]} and {table.name}"
                seen_pinned[tag.lower()] = table.name
            for lcid, tag, *_note in table.regions:
                assert not lcid or lcid not in seen_lcids, f"{lcid:#x} is in {seen_lcids[lcid]} and {table.name}"
                seen_lcids[lcid] = table.name
                assert not tag or tag not in seen_tags, f"{tag} is in {seen_tags[tag]} and {table.name}"
                seen_tags[tag] = table.name
                keys = [f"{lcid:X}" if lcid else "", tag]
                assert any(key.lower() in asked for key in keys if key), \
                    f"{table.name}: the region {lcid:#x} {tag!r} has no row in the case table"
                # A region whose names Excel was asked for (not only its day period) shows its table's names.
                for key in keys:
                    other = names_of(by_code, asked[key.lower()]) if key and key.lower() in asked else None
                    if other is not None:
                        assert other == table_names[table.name], f"{table.name}: {key} shows other names"


def literal(text):
    """A Rust string literal: what is invisible, a separator or a combining mark is written as an escape."""
    out = ['"']
    for char in text:
        if char in '"\\':
            out.append("\\" + char)
        elif unicodedata.category(char)[0] in "ZCM" or ord(char) < 32:
            out.append(f"\\u{{{ord(char):x}}}")
        else:
            out.append(char)
    out.append('"')
    return "".join(out)


def array(items):
    return "[" + ", ".join(literal(item) for item in items) + "]"


def main():
    by_code = rows()
    known = set()
    table_names = {}
    out = [
        "// @generated by tests/excel/names.py from tests/cases.tsv, Excel's own output. Do not edit.",
        "",
        "use super::Names;",
        "",
    ]
    for language in LANGUAGES:
        for table in language.tables:
            found = {}
            for tag in table.pinned:
                names = names_of(by_code, tag)
                assert names, f"{table.name}: no rows for the tag {tag}"
                found[tag] = names
                known.add(tag)
            first, *rest = found
            for tag in rest:
                assert found[tag] == found[first], f"{table.name}: {tag} shows other names than {first}"
            names = found[first]
            table_names[table.name] = names
            out += [f"/// {table.doc}", "#[rustfmt::skip]",
                    f"{'pub ' if table.public else ''}static {table.name}: Names = Names {{"]
            for field in ("months", "short_months", "days", "short_days", "day_periods"):
                out.append(f"    {field}: {array(names[field])},")
            out += ["};", ""]
    check_regions(by_code, table_names)
    for code in by_code:
        for pattern in (MONTHS, DAYS):
            match = pattern.match(code)
            if match and match.group(1) not in known and match.group(1).lower() not in {k.lower() for k in known}:
                sys.exit(f"the tag {match.group(1)} is pinned (`{code}`) and no table of names.py claims it")
    out += [
        "/// A region whose names differ from its language's default: LCID (low word), text tag, table.",
        "#[rustfmt::skip]",
        "pub(super) static REGIONS: &[(u16, &str, &Names)] = &[",
    ]
    for language in LANGUAGES:
        for table in language.tables:
            for lcid, tag, *note in table.regions:
                out.append(f'    ({lcid:#06x}, "{tag}", &{table.name}),' + (f" // {note[0]}." if note else ""))
    out += [
        "];",
        "",
        "/// The default table of each language: primary language id (low ten bits of an LCID), tag, table.",
        "#[rustfmt::skip]",
        "pub(super) static LANGUAGES: &[(u16, &str, &Names)] = &[",
    ]
    for language in LANGUAGES:
        out.append(f'    ({language.primary:#04x}, "{language.tag}", &{language.default.name}),')
    out += ["];", ""]
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text("\n".join(out), encoding="utf-8")
    print(f"{sum(len(language.tables) for language in LANGUAGES)} tables in {OUT}")


if __name__ == "__main__":
    main()
