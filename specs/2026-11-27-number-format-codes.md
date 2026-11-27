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
| `NumberFormat::parse(code)` | up to four sections; `Err` for a code Excel cannot read: Excel shows it as General, and the caller does (`NumberFormat::general()`) |
| `format_number(value, &FormatOptions)` | the section the value takes, then `Formatted` |
| `format_text(text)` | through the text section, the text as it is without one |
| `date_kind()`, `is_date_format(code)` | `Date`, `Time`, `DateTime` or `Duration` (`[h]`) when a positive number shows as one, for a reader that types cells |
| `builtin_code(id, Locale)` | a built-in id (a style with no `numFmt` record) to its code; `None` shows as General |
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
| conditions (`[>=100]`) | the first section whose condition holds; else the third when the first two carry one (none: overflow), else the second, else General for one section | dropped only by a section that takes negatives alone: `[<0]`, `[<-10]`, `[=-5]`, not `[<=0]` or `[<100]` |

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
| fraction: the last convergent of the continued fraction with a denominator within the placeholders (no semiconvergent), or the fixed denominator; a whole part when placeholders precede the numerator; a zero numerator blanks the fraction | `# ??/??`: π → `3  1/7 `, 0.999 → `1      ` |
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
| `y`, `yy` two digits; `yyy` and more, and `e`, four; `mmm`, `mmmm`, `mmmmm`, `ddd`, `dddd` English names | |
| `AM/PM` shows `AM`/`PM` whatever its case; `A/P` the letter as written | `a/p` → `p` |
| `[h]`, `[mm]`, `[ss]`: the whole duration in that unit | `[h]:mm:ss`: 1.5 → `36:00:00` |
| `[$-F800]`, `[$-x-sysdate]`: the long date `dddd, mmmm d, yyyy`; `[$-F400]`, `[$-x-systime]`: the time `h:mm:ss AM/PM` | |

### Built-in ids (en-US)

Ids 0-22 and 37-49 of 18.8.30, with the en-US Short Date `m/d/yy` for 14 (and 22), `mm:ss.0` for 47. The ids of
East Asian and Thai Excel take the en-US code Excel takes for them: 27-31, 36, 50-58, 71, 72 → 14; 32-35, 77 →
21; 59-70 → 1-4, 5-8, 9-10, 12-13; 73-75 → 15-17; 76, 78 → 20, 22; 79-81 → 45-47. Ids 23-26 and unknown ones are
General.

### Locale

The text is en-US: a decimal point, a comma between thousands, English names. `FormatOptions::locale` is
`Locale::EnUs` only; another locale adds a variant with its separators, names, short and long date and time
codes, and built-in codes, with no API change.

### What we do not do

- A `[$-40C]` tag that asks for French month names shows English ones; only the currency symbol of a tag is used.
- `[$-F400]` and id 14 follow the operating system in Excel: the en-US defaults are written above.
- General narrower than 11 characters (Excel shortens it to the column width): the render asks for it later.
- `[DBNum]`, `[NatNum]`, `B1`/`B2` calendars, `g` eras: ignored or literal.

## Plan

One pull request (#365, pull request 4):

- The crate, its unit tests, and one table-driven test over `crates/oxoffice-numfmt/tests/cases.tsv`: 599 cases
  (528 + a second round of 71 for what the first left open), each a code, a value, a date system, Excel's text
  (its CSV export where `Range.Text` depends on the column width) and, where we differ on purpose, our text and
  why. The table and the workbooks it was read from are built by `handoff/xlsx/numfmt/cases.py` and `pin.py`
  (outside the repository).
- `oxoffice-chart` formats through it: dates, fractions, sections and conditions reach axes and labels, General
  is Excel's, and `c:date1904` of the chart space sets the date system (`CT_ChartSpace::date_system`,
  `render_chart` takes it). The diagnostic "not supported and its labels use General" now names only codes Excel
  cannot read.

## Links

- #365 (spreadsheets, first set), pull request 4 of its plan.
- ECMA-376 Part 1, 18.8.30 (`numFmt`) and 18.8.31 (format codes).
