---
date: 2026-11-27
status: accepted
scope: xlsx
---

# Number format codes to the text Excel displays

## Context

A spreadsheet cell holds a number and a format code; what a user reads is the text Excel makes of both. Three
parts of oxoffice need that text (#365):

- the xlsx text, Markdown and CSV export (pull request 5 of #365);
- the xlsx render of milestone 2;
- the chart axes and data labels of Word and PowerPoint (`oxoffice-chart`). Its formatter took numeric sections
  only: a date axis such as the corpus's `[$-1009]yyyy/mm/dd` was drawn as serial numbers, with a diagnostic, and
  General was Rust's float text (`0.30000000000000004`, `100000000000000000000`).

## Decision

**A shared crate, `oxoffice-numfmt`, format-neutral (it depends on `thiserror` only), turns a code as stored in a
file (ECMA-376 Part 1, 18.8.31, the invariant en-US form) and a value into Excel's text. Excel is the reference:
every rule below was checked in Excel for Mac 16.115 (en-US) on the case table, except where a row says otherwise.**

| API | What it does |
| --- | --- |
| `NumberFormat::parse(code)` | up to four sections; `Err` for a code Excel cannot read, or longer than 255 characters (Excel repairs it to General on open): Excel shows it as General, and the caller does (`NumberFormat::general()`) |
| `format_number(value, &FormatOptions)` | the section the value takes, then `Formatted` |
| `format_text(text)` | through the text section, the text as it is without one |
| `date_kind()`, `is_date_format(code)` | `Date`, `Time`, `DateTime` or `Duration` (`[h]`) when a positive number shows as one, for a reader that types cells |
| `builtin_code(id, Locale)` | a built-in id (a style with no `numFmt` record) to its code; `None` shows as General |
| `shows_unknown_names()` | the code is tagged with a language `names` holds no table of (`[$-40D]mmmm`, Hebrew) and shows month names, day names or a day period: this crate writes them in English, Excel in that language |
| `names` | the month names, day names and day periods of the languages below, by region, shared with Word's date controls (`oxoffice-docx-oxml` depends on this crate for them) |
| `FormatOptions` | `date_system` (`Date1900`, `Date1904`), `locale` (`EnUs` only, `non_exhaustive`) |

`Formatted` carries what the text export and a render need, without layout code:

| Field | Meaning |
| --- | --- |
| `text` | one ordinary space per `_x`, nothing for `*x`: Excel's CSV export, width-independent |
| `color` | `[Red]` or `[Color10]`: N of `[ColorN]` (1-56) and its indexed colour N + 7, from the workbook palette |
| `fill` | `*x`: the character and the byte offset where the render repeats it to the cell width |
| `spaces` | `_x`: the byte offset of each space and the character whose width it takes |
| `overflow` | Excel fills the cell with `#`: `text` is `#####`, `fill` is `#` |

Booleans and errors are not formatted: Excel shows `TRUE`, `FALSE` and the error code whatever the format, so the
caller writes them as they are.

The crate has no CLI or Python surface of its own: the displayed text reaches them through the xlsx read API
(pull request 5), and through chart renders.

### Sections and the minus sign

| Code | Which section a number takes | Minus sign |
| --- | --- | --- |
| one section | all | shown, unless the number rounds to zero (`0` shows -0.4 as `0`) |
| two | `>= 0` the first, `< 0` the second | dropped by the second |
| three | `> 0`, `< 0`, `= 0` | dropped by the second |
| a text section | the fourth, or the last when it holds `@`; numbers skip it, and a text-only code shows numbers as General | |
| conditions (`[>=100]`) | the first section whose condition holds, a first section with none taking positives (`[>0]`) when the second has one (`0;[>100]"big"` shows 200 as `200`); else the third when the first two carry one (none: overflow), else the second, else General for one section | dropped only by a section that takes negatives alone: `[<0]`, `[<-10]`, `[=-5]`, not `[<=0]` or `[<100]` |

A literal-only or fraction section shows the minus of a nonzero value even when nothing else is shown
(`[>0]"pos";[>10]"big";"other"` shows -3 as `-other`, `# ?/?` shows -0.01 as `-0    `).

### Numbers

| Rule | Example |
| --- | --- |
| a value keeps 15 significant digits, then rounds half away from zero | `0.00`: 1.005 → `1.01`, 2.675 → `2.68` |
| `0` pads with zero, `?` with a space, `#` with nothing; extra integer digits go on the first placeholder | `???.???`: 1.5 → `  1.5  ` |
| decimals: digits up to the last nonzero one or the last `0` placeholder; the point always shows | `#.##`: 5 → `5.`, 0 → `.` |
| a comma between integer placeholders groups by three; after the last placeholder it divides by 1000; between decimals it is ignored | `0.0,,"M"`, `#0.0,0`: 1234.5 → `1234.50` |
| each `%` multiplies by 100 | `0%%`: 0.5 → `5000%%` |
| no integer placeholder: the integer digits go before the point | `.00`: 12.5 → `12.50` |
| exponent: one digit before the point, or a multiple of the integer placeholder count (engineering); `E+` always signs, `E-` only negatives; lowercase `e` is refused (General) | `##0.0E+0`: 0.5 → `500.0E-3` |
| fraction: the last convergent of the continued fraction with a denominator within the placeholders (no semiconvergent), or the fixed denominator; a whole part when placeholders precede the numerator; a zero numerator blanks the fraction; a numerator beyond 15 digits is an overflow (Excel shows `?/100` with 1E+37 as `#####`; the limit is ours) | `# ??/??`: π → `3  1/7 `, 0.999 → `1      ` |
| General: 11 characters besides the sign, no trailing zero; 6 significant digits in scientific form from 1E+11, and below 1E-5 when the fixed form needs more than 9 decimals; subnormal doubles are 0 | 123456789012 → `1.23457E+11`, 1E-9 → `0.000000001` |
| a text longer than 255 characters is an overflow (Excel shows `0` with 1E+300 as `#####`; the exact limit is ours) | |
| literals: quoted, `\x`, and any other character as itself; `[$€-40C]` is `€`; unknown brackets (`[DBNum1]`) are ignored | |

### Dates and times

| Rule | Example |
| --- | --- |
| 1900: serial 1 is 1900-01-01, 60 is 1900-02-29, 0 is 1900-01-00 (a Saturday); negative: overflow | `yyyy-mm-dd`: 0 → `1900-01-00` |
| 1904: serial 0 is 1904-01-01; a negative value shows its magnitude after a minus | `h:mm`: -0.5 → `-12:00` |
| after 9999-12-31 (2958465, 2957003): overflow | |
| the value rounds to the second, or to the decimals shown (`.0` to `.000`), before the date is taken, in decimal (12:00:00.4999999999968 is noon) | `m/d/yyyy`: 45306.9999999 → `1/16/2024` |
| `m`/`mm` is the minute right after an hour or right before a second (literals between them aside), the month elsewhere | `h "h" m "m"` → `17 h 5 m` |
| `y`, `yy` two digits; `yyy` and more, and `e`, four; `mmm`, `mmmm`, `mmmmm`, `ddd`, `dddd` the names of the locale (English), or of the tag's language | |
| `aaa`, `aaaa` (lower case): the weekday as `ddd`, `dddd`, under every tag and none (East Asian Excel's own spelling: `[$-804]aaaa` is `星期三`); other runs of `a` are literals | `[$-411]aaa`: 水 |
| `AM/PM` shows `AM`/`PM` whatever its case; `A/P` the letter as written | `a/p` → `p` |
| under a language tag, every spelling (`AM/PM`, `am/pm`, `A/P`, `a/p`) shows the designator of the tag's region: `AM` (en-US, fr-FR, de-DE...), `am` (en-GB), `a.m.` (fr-CA), `a. m.` (es-MX) | `[$-809]h A/P`: 5 am |
| `[h]`, `[mm]`, `[ss]`: the whole duration in that unit | `[h]:mm:ss`: 1.5 → `36:00:00` |
| `[$-F800]`, `[$-x-sysdate]`: the long date `dddd, mmmm d, yyyy`; `[$-F400]`, `[$-x-systime]`: the time `h:mm:ss AM/PM` | |
| `[$-40C]` or `[$-fr-FR]`: the names and day period of that language and region (next section); the tag may stand anywhere in its section, and only the first section's counts: it speaks for every section, and a tag of another section changes nothing | `[$-407]mmmm;[$-40C]mmmm`: 30000 → `Februar` |
| two language tags in a date section: Excel cannot read the code and shows General (the one shape asked); any other section takes its first tag, our choice | `[$-40C][$-407]mmmm` → `45306` |

### Built-in ids (en-US)

Ids 0-22 and 37-49 of 18.8.30, with the en-US Short Date `m/d/yy` for 14 (and 22), `mm:ss.0` for 47. The ids of
East Asian and Thai Excel take the en-US code Excel takes for them: 27-31, 36, 50-58, 71, 72 → 14; 32-35, 77 →
21; 59-70 → 1-4, 5-8, 9-10, 12-13; 73-75 → 15-17; 76, 78 → 20, 22; 79-81 → 45-47. Ids 23-26 and unknown ones are
General.

### Locale

The text is en-US: a decimal point, a comma between thousands, English names. `FormatOptions::locale` is
`Locale::EnUs` only; another locale adds a variant with its separators, names, short and long date and time
codes, and built-in codes, with no API change.

A locale tag changes the names of a date code and nothing else (`[$€-40C]#,##0.00` is `€1,234.57`). The tag is an
LCID in hexadecimal (`40C`; its low word counts for a high word of 0 or 1, `1040C` being French; `2040C` showed
English and reads as English, a higher high word was not asked and counts as a language with no table; a tag with
no primary language, `[$-400]`, names none) or a language tag (`fr-FR`, `fr`; case does not matter; a language alone,
`de`, is no LCID). A neutral LCID (`[$-A]`, `[$-16]`) reads as the tag with no region (`[$-es]`, `[$-pt]`): our
choice, since Excel showed those tables other than Spain's and Brazil's and no producer writes a neutral LCID.
Excel for Mac, asked for 32 tags in rounds 4 and 5 and 34 more in rounds 6 and 8 (`tests/cases.tsv`; each with its
twelve months, seven weekdays and day period, except en-NZ, en-IE and en-ZA, asked for the day period only), shows
names that depend on the region, not only the language:

| Language | Regions asked, with their own table | Same table as |
| --- | --- | --- |
| English | en-US, en-GB (`am`, `pm`) | en-AU: en-GB; en-CA, en-IN: en-US; the day period only: en-NZ, en-ZA as en-US, en-IE as en-GB (their names are not pinned: they follow the other English regions) |
| French | fr-FR, fr-CA (`juill.`, `a.m.`) | fr-BE, fr-CH, fr-LU: fr-FR |
| German | de-DE, de-AT (`Jänner`, `Jän`) | de-CH, de-LU: de-DE |
| Spanish | es-ES, es-MX, es-AR, es-CO, es-CL, es-US, and `es` | es-ES traditional sort (`40A`): es-ES |
| Italian | it-IT | it-CH |
| Portuguese | pt-BR, pt-PT, and `pt` | |
| Dutch | nl-NL | nl-BE |
| Japanese | ja-JP | |
| Chinese | zh-CN (`一月`, `周三`), zh-TW (`9月`, `週三`), zh-HK (`九月` for the short month too, `週三`), zh-SG (`九月`, `周三`), `zh` and `zh-Hans` (`一月`, `1月`, `週三`) | zh-MO and `zh-Hant`: zh-TW; another region: zh-CN |
| Korean | ko-KR | |
| Russian | ru-RU (the day period is `AM`, `PM`) | ru-MD |
| Polish | pl-PL (`AM`, `PM`) | |
| Swedish | sv-SE (`fm`, `em`) | sv-FI |
| Turkish | tr-TR (`ÖÖ`, `ÖS`) | |
| Catalan | ca-ES (`a.\u{202f}m.`) | ca-ES-valencia |
| Arabic | ar-SA (`يناير`; the short names are the full ones) and its eleven regions Egypt, Libya, Algeria, Morocco, Tunisia, Oman, Yemen, Kuwait, UAE, Bahrain, Qatar; the Levant, ar-IQ, ar-SY, ar-JO, ar-LB (`كانون الثاني`, `شباط`) | `ar`: ar-SA; the neutral LCID `[$-1]` shows English |

The full names agree in every region asked of the languages of rounds 4 and 5, except `Jänner`, and with the tables of Word's date controls
(`oxoffice-docx-oxml`, which now reads them from this crate). Short names and the day period differ by region
(`ene.` in Spain and Colombia, `ene` in Argentina and the United States, `set` for September in `es`; `a.\u{202f}m.`
in Spain, `a.\u{a0}m.` in Colombia and Chile, `a. m.` in Mexico). Where a language is tagged with a region not
asked, it takes the table of its most widespread region (en-US, fr-FR, de-DE, es-ES, pt-BR) or, for a tag with no
region, as asked. The one-letter month is the first letter of the full name, as written (`j`, `s`, `9`, `九`).

A month name does not change next to a day number: Excel for Mac writes `15 январь` and `15 styczeń`, not the genitive
(`января`, `stycznia`) that Russian and Polish system data holds, whatever the codes (`d mmmm`, `dd mmmm yyyy`,
`mmmm d`, `d-mmm-yy`, `dddd, d mmmm yyyy`, a literal between): every month of both, five codes (round 6).
Digits stay Western in Arabic (`4 سبتمبر 2024`). The day period in the morning and in the afternoon, and at
midnight and noon, are the two designators whatever the spelling (`AM/PM`, `am/pm`, `A/P`, `a/p`).

### What we do not do

- A tag of a language with no table (`[$-40D]`, Hebrew) shows English names and `AM`/`PM`.
  `shows_unknown_names()` says so, and the chart keeps such a code unsupported (General, with its diagnostic). Every
  language Excel was asked of in rounds 4 to 8 has its table, so a file saved by a producer in one of the sixteen
  languages reads right; one more language is one Excel round and an entry in `tests/excel/names.py`.
- Eras: `g`, `gg`, `ggg` and the era year of `e` under a Japanese tag (`[$-411]ggge"年"m"月"d"日"` is `令和6年9月4日`,
  `e` is `6`), and `g`, `gg`, `ggg` showing nothing under another tag, are a calendar feature: this crate shows the
  letters and the Gregorian year (rows r7e-*, each with its reason).
- A region of English, French, German, Spanish or Portuguese that Excel was not asked for takes the table of the
  widespread region of its language: its short names and day period can differ.
- `[$-F400]` and id 14 follow the operating system in Excel: the en-US defaults are written above.
- General narrower than 11 characters (Excel shortens it to the column width): the render asks for it later.
- `[DBNum]`, `[NatNum]`, `B1`/`B2` calendars, `g` eras: ignored or literal.

## Plan

Pull request 4 of #365:

- The crate, its unit tests, and one table-driven test over `crates/oxoffice-numfmt/tests/cases.tsv`: 621 cases
  in three rounds (526, then 71 and 24 for what the rounds before and the review left open), each a code, a
  value, a date system, Excel's text (its CSV export where `Range.Text` depends on the column width) and, where
  we differ on purpose, our text and why. `tests/excel/` holds the scripts that build each round's workbooks
  (`cases.py`) and pin Excel's text into the table (`pin.py`); its README says how a round is run.
- `oxoffice-chart` formats through it: dates, fractions, sections and conditions reach axes and labels, General
  is Excel's, and `c:date1904` of the chart space sets the date system: `render_chart` takes the chart space,
  so no caller can forget it, and `NumberFormat::format_value` takes the options. The diagnostic "not
  supported and its labels use General" names the codes Excel cannot read and those that ask for names in
  a language `names` holds no table of.

Pull request 4b (names under a language tag): the name tables of `oxoffice-docx-oxml` move to `oxoffice-numfmt::names`
and gain the short names, the day periods and the regions of Excel's own output; Word's date controls read them from
there and show what they showed (their tests, and the limitation `docx-date-control-names`, are unchanged: short
names other than English stay refused there on purpose). Two rounds of Excel cases check it: round 4 (484 cases: 21
LCIDs, each with its twelve months, seven weekdays and day period; 18 text tags; nine languages with no table; the
position and number of tags) and round 5 (241: each spelling of the day period alone, six more regions, `es` and
`pt`, three more English regions for the day period, the high word of an LCID, which sections a tag speaks for). The chart keeps its diagnostic for a language with
no table only.

Pull request for #707 (the languages with no table): Excel rounds 6, 7 and 8 (1,449 cases) ask for Japanese, Chinese
(five regions and the tags with no region), Korean, Russian, Polish, Swedish, Turkish, Catalan and Arabic (sixteen
regions): their months, weekdays and day period per region, whether a month changes form next to a day number (no),
full date formats as a user picks them, the tags as text and as neutral LCIDs, and `aaa`, `aaaa` and the eras. The
tables are no longer written by hand: `tests/excel/names.py` writes `src/names/tables.rs` from the pinned rows, and
`names` finds a region by its LCID or its tag in that data (`REGIONS`) before the language's default. The chart's
diagnostic is kept for a language with no table only, and the limitation `xlsx-date-names-language` is removed.

## Links

- #365 (spreadsheets, first set), pull request 4 of its plan.
- ECMA-376 Part 1, 18.8.30 (`numFmt`) and 18.8.31 (format codes).
