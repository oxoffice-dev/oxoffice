---
date: 2026-11-27
status: accepted
scope: xlsx
---

# Workbooks: open and save without loss, read and edit cells, a new workbook

## Context

[#365](https://github.com/oxoffice-dev/oxoffice/issues/365) adds `.xlsx` as the third family. Its first set reads any
workbook saved by Excel, Google Sheets or LibreOffice, edits cells, formulas and styles without losing anything, and
creates a workbook. The case behind it is tensorbee/rdocx issue 281: a review pass over a Google Sheets workbook that
changes cells, edits a column driven by one shared formula, and leaves the reviewers' comments alone.

This spec covers pull requests 3 (open and save), 5 (read), 6 (edit cells) and 7 (new workbook) of the plan comment.
Pull requests 2 (corpus) and 4 (`oxoffice-numfmt`, its own spec) run in parallel; what this work expects from them is
under the plan. Producer facts come from LibreOffice 26.2 (probed here), a Google Sheets export
(`handoff/xlsx/probes/google-probe.xlsx`) and XlsxWriter 3.3.3; Excel's are marked *to check on the corpus*.

## Decision

### Taken with the plan

- Our own family on `oxoffice-opc`, no third-party workbook crate; `oxoffice-sml` stays the chart workbook writer.
- Loss-aware: what is not modelled is kept as is, a sheet not edited is written back byte for byte; atomic save,
  never over the input.
- No evaluator in this set: a changed formula or value sets `fullCalcOnLoad` and removes `calcChain.xml`, and the API
  reports cached values as stale. The formula lexer lives in `oxoffice-xlsx-formula`, which the parser, the evaluator
  and row insertion extend later.
- Comments read only (notes and threads); writing them is step 8. Python: our own shape, consistent with
  `oxoffice.docx` and `oxoffice.pptx`, no openpyxl parity. Rust facade, then CLI, then Python, in each pull request.

### 1. Crates

| Crate | Role | Depends on (oxoffice) |
| --- | --- | --- |
| `oxoffice-numfmt` (PR 4, shared) | number format code + value -> displayed text, section colour, `_` and `*` hints; built-in ids; date serials 1900 / 1904 | none |
| `oxoffice-xlsx-formula` | lexer, stored <-> displayed forms, reference mover, syntax check, function table (prefixes, arity); later the rest of the parser, the dependency graph, the evaluator, reference updates for inserted rows | none |
| `oxoffice-xlsx-oxml` | the SpreadsheetML types (`CT_*`), the worksheet row index, the splice writer | `core` |
| `oxoffice-xlsx` | the facade: `Workbook`, `Worksheet`, cells, styles, comments, CSV, the new workbook | `opc`, `core`, `drawing`, `numfmt`, `xlsx-oxml`, `xlsx-formula` |
| `oxoffice-xlsx-cli` | the `oxoffice xlsx` commands (library) | `xlsx`, `cli-support` |
| `oxoffice-xlsx-py` | the `oxoffice.xlsx` bindings (library), registered by `oxoffice-py` as `oxoffice.xlsx._native` | `xlsx`, `py-support` |

The dependency rule gains the third family: no shared crate depends on an `-xlsx` crate, and `-xlsx`, `-docx` and
`-pptx` crates never depend on each other. `oxoffice-sml` and `oxoffice-chart` keep no edge to the xlsx family.
`oxoffice-drawing` parses the theme and gains one colour function, `apply_hls_tint`, the SpreadsheetML tint (HLS
luminance, `CT_Color@tint` of ECMA-376 Part 1), beside the DrawingML ones it has. No WebAssembly crate in this set;
the facade's core uses nothing `wasm32` lacks, as docx and pptx, so bindings can follow when a user asks.

### 2. The model

**Parts.** A part is written from the model only when an edit changed it; otherwise its bytes are copied.

| Part | In the model | Written |
| --- | --- | --- |
| `workbook.xml` | `workbookPr` (`date1904`), `sheets`, `definedNames` (print area and titles read), `calcPr`; other children as bytes | spliced: `calcPr`, a new `sheet` |
| `sharedStrings.xml` | parsed at the first `t="s"` read: plain text per item, runs and phonetics kept as bytes | new items spliced before `</sst>`, `count` and `uniqueCount` updated |
| `styles.xml` | `numFmts`, `fonts`, `fills`, `borders`, `cellStyleXfs`, `cellXfs`, `cellStyles`, `colors`; `dxfs`, `tableStyles`, `extLst` as bytes | new records spliced at the end of their list, its `count` updated |
| worksheets | on demand, see below | source bytes with edited rows and children spliced |
| comments, threaded comments, `persons/person.xml`, tables (header row), pivot locations | read only | as read |
| `theme/theme1.xml` | `oxoffice-drawing` theme, read | as read |
| `metadata.xml` | PR 6: `metadataTypes`, the `XLDAPR` `futureMetadata`, `cellMetadata`; the rest as bytes | spliced when a spill formula needs its blocks (section 6) |
| `calcChain.xml` | not read | removed on a save after an edit (section 3) |
| everything else: drawings, VML, charts, pivot caches, external links, `vbaProject.bin`, `customXml`, `docProps`, printer settings | bytes | as read |

**Missing containers.** A container an edit needs and the file lacks is created at its schema position: `numFmts`
(Google writes none) and any other list of `styles.xml`; `sharedStrings.xml` (a workbook without text has none) and
`metadata.xml` (LibreOffice drops it), each with its workbook relationship and content type override.

**A worksheet.** Its first read or edit makes one pass over `sheetData` recording each row's number and byte span,
each shared formula master (`si` -> cell, text, `ref`) and each array and data table range. A row becomes `CT_Row` /
`CT_Cell` when read or edited. An edited sheet saves as its source bytes with each edited row re-serialized in its
span, new rows at their place, and the children an edit changes (`dimension` widened; `cols`, `sheetViews` in PR 7)
spliced at their schema position; the root start tag (declarations, `mc:Ignorable`), every other child and every
row not edited stay bytes. An edit costs its rows plus, once per group, the rows of a shared group it dissolves
(section 6), not the sheet (the docx lesson of #466). Rows and cells without `r` take the next number. Tolerant on
read (any prefix, `1`/`true`, `0`/`false`, `1.0`). On write, an element takes the prefix the root binds to the main
namespace (none under a default namespace, `x:` or `ns0:` when the root binds one), never a fixed one (spec
2026-11-26 met the same question in Word); canonical attribute values (`1`/`0`, the shortest text that reads back the
same double) appear only in what an edit rewrites.

**Indexes only grow.** Shared strings and `cellXfs` (and the fonts, fills, borders and number formats they point to)
are appended, never removed or reordered, so the sheets not edited keep valid indexes. A written text reuses an
identical plain shared string, never a rich one.

**Cells.** The date system (`workbookPr date1904`) is read once and passed to every date conversion.

| Stored | Read as | Written |
| --- | --- | --- |
| no `t` or `t="n"` | number (`1.0` and `1` alike) | number |
| `t="s"` | text; a rich item reads as its plain text (runs kept, not exposed in this set) | new text goes to shared strings |
| `t="inlineStr"` | text from `<is>` | kept while unedited; new text goes to shared strings |
| `t="str"` | formula text result | as read |
| `t="b"` | boolean | `1` / `0` |
| `t="e"` | error: `#NULL!` `#DIV/0!` `#VALUE!` `#REF!` `#NAME?` `#NUM!` `#N/A` `#GETTING_DATA`, newer codes kept as text | a constant error can be written |
| `t="d"` (ISO 8601) | the serial of that date-time in the workbook's date system | kept while unedited; edits write serials |
| `cm`, `vm` (dynamic array and value metadata, `metadata.xml`) | kept | dropped with the content they describe when the value or formula changes; a spill formula writes its `cm` |
| `xr:uid`, `x14ac:*`, any other attribute | kept | as read; none added |

**Escaped characters** (`ST_Xstring`, ECMA-376 Part 1, 22.9.2.19), in shared and inline strings, rich runs, comments
and every other value of that type the set reads or writes (sheet names, format codes, header and footer text): read
decodes `_xHHHH_` (Excel writes a carriage return as `_x000D_`); write stores a character XML cannot carry and a
carriage return as `_xHHHH_`, and a literal `_xHHHH_` as `_x005F_xHHHH_`, as the pptx facade stores run text.

**Formulas.**

| Kind | Read | Edit |
| --- | --- | --- |
| plain `<f>` | its text | replaced |
| shared, master (`t="shared" ref si` with text) | its text | dissolves the group (section 6) |
| shared, member (`t="shared" si`, no text) | the master's text moved from the master to the cell by `oxoffice-xlsx-formula` (relative parts move, `$` parts stay, a reference leaving the grid becomes `#REF!`); a member with its own text keeps it | dissolves the group |
| array (`t="array" ref`), dynamic or not | text and range | a partial edit is refused; a clear or write of the whole range removes it |
| data table (`t="dataTable"`) | kept, range read | refused like an array |

**Stored and displayed forms.** Formula text is exchanged without the `=` (a write accepts one) in Excel's displayed
form, which the lexer maps to the stored form and back: future functions take and lose `_xlfn.` (and `_xlws.`) from
the complete MS-XLSX table, with the later functions it lacks (`XMATCH`, `GROUPBY`...), and a prefixed name the table
lacks keeps its prefix on read so it comes back unchanged; LET and LAMBDA parameters take `_xlpm.` on their
declarations and on every use, scope tracked (nested LET, a parameter that shadows a defined name);
`_xlfn.ANCHORARRAY(x)` is `x#` and `_xlfn.SINGLE(x)` is `@x`. Table references read as stored
(`Expenses[[#This Row],[Quantity]]`: Excel shows `[@Quantity]` inside the table, which depends on the cell); a written
`Table[@Column]` is stored `Table[[#This Row],[Column]]`. Reading then writing a formula gives back its stored text,
for every corpus formula. The stored form is readable in Rust, Python
(`Cell.stored_formula`) and the CLI (`stored_formula` in `--json`), and writable as is (`stored=True`, section 6).

### 3. Recalculation and stale values

A formula cell's cached value is **stale** when:

1. the workbook was edited through the API since it was opened: any write to a worksheet, the styles, the shared
   strings or the workbook part (value, formula, clear, style, number format, width, height, freeze, a new sheet).
   Style and width edits count because `CELL` and `INFO` read them;
2. or the file as read asks for a recalculation: `calcPr fullCalcOnLoad="1"` (XlsxWriter writes it over placeholder
   results), or `calcMode="manual"` with `calcOnSave="0"`;
3. or its own formula carries `ca="1"` (calculate cell; Excel marks volatile formulas so, *to check on the corpus*).

Otherwise it is what the producer computed at its last save; without `<v>` there is no cached value.

A save under rule 1 sets `fullCalcOnLoad="1"` on `calcPr` (inserted at its schema position when absent; Google writes
`<calcPr/>`) and removes `xl/calcChain.xml`, its relationship and its content type; the cached values stay. Excel
recalculates on open, Google always does. A written formula has no `<v>` and no `t`. An unedited workbook saved
without `drop_cached_values` (below) is byte for byte the file it was read from.

LibreOffice ignores `fullCalcOnLoad` by default (probed: "Recalculation on file load" is "never"). It computes the
formulas without `<v>` and every shared-formula cell, and shows the other old values until a recalculation (that
setting, or Ctrl+Shift+F9), members of a group an edit dissolved included. LibreOffice ranks after Excel and Google
(maintainer), so a save option serves it rather than a dependency walk: `drop_cached_values` (Rust `SaveOptions`,
taken by every save, `to_bytes` and the encrypted ones included; Python the same keyword; CLI `--drop-cached-values`)
makes the save a recalculating one even without an edit (`fullCalcOnLoad` set, chain removed) and drops the `<v>` and
`t` of every formula and its array range, rewriting the rows that hold formulas in every sheet; LibreOffice then
computes them on load, and a function it lacks (`LAMBDA`, `GROUPBY`) shows an error where the cached value was. The
README says so; it is no known limitation. The dependency graph comes with the evaluator of the second set, which
writes correct values and makes the option unnecessary.

### 4. Addressing

- **A1 strings everywhere**: `B3` and `$B$3`, ranges `A1:C10`, whole columns `A:C` and rows `2:5` (clipped to the
  used range when read), letters in any case; the CLI also takes `'Q1 data'!B3`. Limits: 1,048,576 rows, 16,384
  columns (`XFD`).
- **Positions**: `(row, column)`, 0-based in Rust and Python, like every index of both packages (`slides[0]`,
  `paragraphs[0]`) and like the nested lists of a bulk read; the spreadsheet's own 1-based numbering is the A1
  string. Python takes `str | tuple[int, int]` wherever it takes a cell. The CLI is 1-based like every CLI number.
- **Sheets**: by name (exact, then case-insensitive, as Excel compares sheet names) or position: 0-based in Rust and
  Python (`wb.sheets[0]`, negative from the end), 1-based in the CLI. `--sheet` takes a name first, then a number.
- **Handles**: a Python `Worksheet` holds its sheet's relationship id, valid through every edit of this set. Cells are
  snapshots read by address, never live handles: nothing goes stale, and `ws.cell("B3").value = 5` raises
  `AttributeError` instead of writing nowhere.

### 5. Read (PR 5)

| What | Rust (`oxoffice_xlsx`) | CLI (`oxoffice xlsx`) | Python (`oxoffice.xlsx`) |
| --- | --- | --- | --- |
| workbook | `Workbook::open`, `from_bytes`, `open_encrypted`; `sheets()`, `date_system()`, `recalculation_needed()` | `inspect [--json]` | `Workbook(path)`, `Workbook.open`, `from_bytes`, `open_encrypted`; `.sheets`, `.sheet(name)`, `.date1904`, `.recalculation_needed` |
| sheet | `name()`, `position()`, `state()` (`visible`, `hidden`, `veryHidden`), `used_range()` | in `inspect` | `.name`, `.index`, `.state`, `.used_range` |
| cell | `cell(ref) -> Cell` | `cells FILE RANGE [--sheet S] [--json]` | `ws.cell(ref) -> Cell` |
| range | `values(range)`, `cells(range)` (row-major, empty cells skipped) | `cells` | `ws.values(range, text=False)` (one native call, nested lists), `ws.cells(range)` |
| merges | `merged_ranges()` | `cells --json`, `text --json` | `.merged_ranges` |
| comments | `comments()` | `comments [--json]` | `.comments` |
| page setup | `page_setup()` | `inspect --json` | `.page_setup` |
| text | `to_markdown()`, `to_csv(options)` in the facade | `text [--json \| --markdown] [--sheet S]...`, `csv` | `wb.to_markdown()`, `ws.to_csv(...)` |

**Used range**: the smallest range holding every cell with a value or a formula (`None` when empty); not `dimension`
(Google writes none), not the rows (Google pads empty rows to 1000).

**`Cell`**: `ref` (Rust `address`), `row`, `column`, `value` (number, text, boolean, error or empty; for a formula,
its cached value), `formula` (`None` or the displayed form), `stored_formula`, `formula_kind` (`shared`, `array`,
`data_table` or `None`), `array_range`, `stale`, `number_format` (the code, built-in ids resolved by
`oxoffice-numfmt`), `text` (displayed), `style`. Python numbers are `float` (Excel stores doubles; producers write `1`
and `1.0` alike); an error is a `CellError`, a `str` subclass (`== "#N/A"` works, `isinstance` tells it from text);
`Cell.datetime` is a `datetime.datetime` for a number under a date or time format, else `None`.

**Displayed text**: `oxoffice-numfmt`, en-US, as in a column wide enough (no `####`, `General` at 11 characters);
booleans `TRUE` / `FALSE`, errors their code, text through the format's text section.

**Resolved style** (`ResolvedStyle`): font (name, size, bold, italic, underline, strike, vertical alignment, colour),
fill (pattern and its two colours; a gradient's type, degree and stops), borders (four sides and diagonal: style,
colour), alignment (horizontal, vertical, wrap, shrink, indent, rotation, reading order), protection, style name:

- The cell's `cellXfs` record gives the number format, font, fill, border, alignment and protection (Excel writes
  complete records); `apply*` attributes are kept but not used on read; `xfId` gives the cell style name. A missing
  `s` or one out of range is record 0. A font that leaves a property out (Google's second font has no `sz`) takes it
  from font 0 (*to check on the corpus*).
- A colour resolves to RGB, keeping its source (`rgb`, `theme` + `tint`, `indexed`, `auto`). `theme` indexes the
  scheme as Excel does: 0 `lt1`, 1 `dk1`, 2 `lt2`, 3 `dk2`, 4-9 `accent1`-`accent6`, 10 `hlink`, 11 `folHlink`
  (every probe's default font is `theme="1"`, which only this order reads as dark text); a system colour takes its
  `lastClr`; `tint` goes through `apply_hls_tint`. `indexed` uses the 64-colour default palette (ECMA-376 Part 1,
  18.8.27) or the file's `indexedColors`, 64 the system foreground, 65 the background. `auto` resolves to `None`. The
  alpha byte of `rgb` is ignored (*to check*: Excel writes `FF`).

**Comments** (`Comment`): `ref`, `kind` (`note` or `thread`), `author`, `date` (ISO 8601 as stored, threads only),
`text`, `resolved`, `replies` (author, date, text). Notes come from the `comments` part related to the sheet (the VML
kept, not read); threads from `threadedComments` (`ref`, `dT`, `personId`, `id`, `parentId`, `done`) with names from
`persons/person.xml`. A note whose author is `tc={id}` is Excel's copy of thread `id` for older versions and is not
listed twice. Ordered by cell, row-major, replies by date.

**Page setup** (`PageSetup`), typed from PR 3 so the render needs no remodel, written back as read (no setter here):
`pageMargins`, `pageSetup` (paper, orientation, scale, fit to pages with `pageSetUpPr fitToPage`, first page number,
order, black and white, draft, comments, errors), `printOptions`, `headerFooter` (six strings with their `&` codes,
flags), `rowBreaks`, `colBreaks`, and the sheet's `_xlnm.Print_Area` and `_xlnm.Print_Titles`; schema defaults fill
absent attributes.

**`text --json`** (schema 1; `sheet` is 1-based like `--sheet`; `cells` holds the cells with a value or a formula):

```json
{"schema": 1, "date1904": false, "recalculation_needed": false,
 "sheets": [{"sheet": 1, "name": "Data", "state": "visible", "used_range": "A1:F16",
   "cells": [{"ref": "A1", "type": "text", "value": "Name", "text": "Name"},
             {"ref": "D2", "type": "number", "value": 1.5, "text": "1.50", "formula": "B2*C2", "stale": false}],
   "merged": ["A14:C14"],
   "comments": [{"ref": "A2", "kind": "note", "author": "Ann", "date": null, "text": "A note",
                 "resolved": false, "replies": []}]}]}
```

`cells --json` adds `stored_formula`, `formula_kind`, `array_range`, `number_format`, `style`. Plain `text`: per
sheet, `# <name>` then each used row's displayed texts joined by tabs. `--markdown`: per sheet, `## <name>` and a
table headed by the column letters, the row number first, so every value keeps its address for an edit; `|` escaped,
line breaks `<br>`.

**`oxoffice xlsx csv FILE [--sheet S] [--raw] [--delimiter C] [--bom] [-o OUT]`**: one sheet (the first visible one
by default), from A1 to the last row and column of the used range, so a field sits at its cell's position; displayed
texts by default (what Excel's own CSV export writes), stored values with `--raw` (shortest double, `TRUE` / `FALSE`,
error codes, date serials). UTF-8 without a BOM unless `--bom`, CRLF line ends, RFC 4180 quoting, standard output
without `-o`. CSV is written and read by hand in the facade (shared by CLI and Python): no new dependency.

### 6. Edit (PR 6)

| Operation | Rust (`WorksheetMut`) | CLI | Python (`Worksheet`) |
| --- | --- | --- | --- |
| value | `set_value(cell, Value)` | `set FILE CELL VALUE [--sheet S] [--text] -o OUT` | `set_value(cell, value)` |
| formula | `set_formula(cell, text, FormulaOptions::default())` | `set ... --formula [--stored]` | `set_formula(cell, text, *, stored=False)` |
| spilling formula | `FormulaOptions { spill: true, .. }` | `set ... --formula --spill` | `set_formula(cell, text, spill=True)` |
| clear | `clear(range, Clear::{Contents, Formats, All})` | edit map | `clear(range, *, contents=True, formats=False)` |
| bulk write | `write(top_left, rows)`, a formula as `CellInput::Formula(text, FormulaOptions)` | edit map | `write(cell, rows)`, a formula as `Formula(text, stored=False, spill=False)` |
| number format | `set_number_format(range, code)` | edit map | `set_number_format(range, code)` |
| style parts | `set_style(range, StylePatch)` | edit map | `set_style(range, *, bold=None, italic=None, ...)` |
| CSV import | `import_csv(top_left, text, CsvImport)` | `import-csv FILE CSV [--sheet S] [--at A1] [--text] -o OUT` | `import_csv(source, *, at="A1", infer=True)` |

- **Values**: number (finite), text (at most 32,767 characters, never read as a formula), boolean, error, empty; a
  date or time writes its serial and, when the cell's format is `General`, the code `yyyy-mm-dd`, `yyyy-mm-dd
  hh:mm:ss` or `hh:mm:ss` (locale-independent, unlike built-in 14). Python maps `int`/`float`, `str`, `bool`,
  `CellError`, `None` (clears the content), `datetime`, `date`, `time`, `timedelta` (`[h]:mm:ss`); a formula in a
  bulk write is an `oxoffice.xlsx.Formula(...)`, so a string is never a formula. `stored` and `spill` reach every
  writer: `set_formula`, the bulk write, the CLI `set` and the edit map (`"stored": true`, `"spill": true`).
- **Formulas**: at most 8,192 characters, and they pass the syntax check of PR 5, the first half of the parser
  (operands, operators, function calls, array constants, `,` between arguments; a `;` outside an array constant is
  refused naming `,`; at most 255 arguments and 64 nested levels). Its function table carries each built-in
  function's minimum and maximum argument count (MS-XLSX's argument rules, checked against the MS-XLS function table,
  whose maximums are the 30 of `.xls`), which the evaluator reuses. So a formula Excel would repair (`IF(B2>0;1;0)`,
  `B2*`, `B2**C2`, `IF(1,2,3,4)`) is refused with a plain message. `stored=True` takes the stored form as is
  (section 2), still checked.
- **Spill**: `<f t="array" ref>` on the anchor cell alone, as XlsxWriter writes it, Excel spilling it on load; `cm`
  points to a `cellMetadata` block whose `rc` names the `XLDAPR` type of `metadata.xml`, found in place or appended,
  the part created when absent (`cm` and `t` 1-based, `v` 0-based, as Excel and XlsxWriter write them).
- **The edit map** (`oxoffice xlsx edit FILE --map EDITS.json -o OUT`): a JSON array applied in order, all or nothing,
  a refusal naming the zero-based entry, like `replace --map`: `{"sheet": "Data", "cell": "B2", "value": 3}`,
  `{"cell": "D2", "formula": "B2*C2"}`, `{"cell": "E2", "date": "2026-01-05"}`, `{"range": "A1:F1", "style":
  {"bold": true, "fill": "FFFF00"}}`, `{"range": "C2:C8", "number_format": "0.00"}`, `{"range": "A20:B30",
  "clear": "contents"}`, and in PR 7 widths, heights and freeze. `sheet` defaults to the entry before it, then the
  first sheet.
- **Shared formulas stay valid.** A content edit (value, formula, clear) of any cell of a shared group dissolves the
  group: every other member becomes a plain formula with its moved text (its row rewritten), the edited cell takes
  its new content. No master moves and no `ref` shrinks, so no reader can place a 1D or 2D group's master wrongly.
- **Refused, with the plain message naming the way out**: a partial edit of an array or data table range; a value in
  a cell a merged range covers (its first cell holds the value; styles are allowed); a header cell of a table whose
  new text differs from the table column name (Excel repairs that; renaming a column comes with the tables API, step
  8); a cell inside a pivot table's location.
- **Kept over an edited cell**: hyperlinks, merges, validations, conditional formats, comments (all outside
  `sheetData`). An edited row's `spans` widens to its cells.
- **Styles are deduplicated.** A style edit builds the cell's record with the changed parts (`apply*="1"` on them, as
  Excel writes) and reuses an identical `cellXfs` record, else appends one; the font, fill, border it needs are reused
  when an identical record exists (compared on what the model reads; a record with unmodelled content only when its
  bytes match), else appended. A number format code reuses a built-in id only when `oxoffice-numfmt` says the id is
  locale-independent, else an identical custom `numFmt`, else a new id from 164 up. Colours are written as `rgb`
  (`FF` + RRGGBB); writing theme colours is not in this set.
- **CSV import** writes the rectangle the CSV covers from `at`; an empty field clears that cell's content. UTF-8 (a
  BOM skipped, another encoding refused naming the first bad byte; Python also takes a `str`). Types inferred
  strictly: `-?[0-9]+(\.[0-9]+)?([eE][+-]?[0-9]+)?` with no leading zero but `0` itself and at most 15 significant
  digits (`007` and a 16-digit card number stay text) is a number, `TRUE` / `FALSE` (any case) a boolean,
  `YYYY-MM-DD[( |T)HH:MM[:SS]]` a date, anything else text, a leading `=` included (no formula from a CSV). `--text` /
  `infer=False` keeps every field text; per-column types wait. The CLI `set` reads its `VALUE` by the same rules.

### 7. New workbook, sheets and grid (PR 7)

- **`Workbook::new()`**, Python `Workbook()`, CLI `oxoffice xlsx new [--sheet NAME]... -o OUT`: `Sheet1` (or the
  names given), Calibri 11 under the Office 2013-2022 theme, one font, the `none` and `gray125` fills, one border, one
  `cellXfs` record, the `Normal` style, `app.xml` naming oxoffice `XX.YYYY` as docx does, a core part with no dates.
- **Deterministic by construction**: the parts are text files under `crates/oxoffice-xlsx/assets/default/`, assembled
  into an `OpcPackage` at run time, whose `write_to` sorts entries and stamps the fixed ZIP date: the same build
  writes the same bytes everywhere. No binary `default.xlsx` is committed.
- **Add a sheet** (`add_sheet(name)`), at the end: a worksheet part (`sheetN.xml`, the first free `N`), its
  relationship, content type and `<sheet>` with the next `sheetId`; `app.xml` drops `HeadingPairs` and `TitlesOfParts`
  as the pptx facade does after a slide list change (Excel writes them again). Excel's name rules: 1-31 characters,
  none of `: \ / ? * [ ]`, no leading or trailing `'`, unique case-insensitively, not `History`. A position renumbers
  `localSheetId` and `activeTab`: it comes with reorder, rename and remove in the second set.
- **Column widths** in characters (`set_column_width("A:C", 20.0)`, 0-255): `cols` spans are split as needed,
  `customWidth="1"`. **Row heights** in points (`set_row_height("2:5", 30.0)`, 0-409): `ht`, `customHeight="1"`, an
  absent row created. Python takes numbers in these units, not EMU (the one-length-argument rule leaves such
  arguments alone); a `Length`, an EMU `int`, is refused with `TypeError`.
- **Frozen panes** (`freeze_panes("B2")`, `None` to remove): the pane written as Excel writes it (`xSplit`, `ySplit`,
  `topLeftCell`, `activePane` `bottomRight` / `topRight` / `bottomLeft`, `state="frozen"`); selections of panes that
  no longer exist are dropped.

### 8. Opening and saving

| Input | Detected by | Outcome |
| --- | --- | --- |
| `.xlsx`, `.xltx`, `.xlsm`, `.xltm` | main part content type | opened; a VBA project and its signature parts kept |
| Strict Open XML (`purl.oclc.org` namespaces) | the workbook relationship or root namespace | opened and kept Strict: one table maps the Strict namespaces and relationship types to their transitional twins on read; edited parts keep the root's namespace, a new part takes the package's |
| encrypted workbook | a compound file (`D0 CF 11 E0`) with an `EncryptionInfo` stream | plain open refused: "the workbook is password-protected: open it with a password"; Agile encryption through `oxoffice-opc` in PR 3, as docx: `open_encrypted`, `from_encrypted_bytes`, `save_encrypted`, `to_encrypted_bytes` |
| `.xls`, `.doc`, `.ppt`, another compound file | a compound file with a `Workbook` or `Book`, `WordDocument` or `PowerPoint Document` stream, or none of them | refused, each with its message: "a legacy Excel 97-2003 workbook (.xls): save it as .xlsx in Excel or LibreOffice first", and the like |
| `.xlsb` | main part `sheet.binary.macroEnabled.main` | refused: "a binary workbook (.xlsb): save it as .xlsx first" |
| `.ods` | ZIP `mimetype` `application/vnd.oasis.opendocument.spreadsheet` | refused: "an OpenDocument spreadsheet (.ods): save it as .xlsx first" |
| a Word or PowerPoint package | main part content type | refused, naming `oxoffice docx` / `oxoffice pptx` |

The package is told by its content, never its extension (today a compound file fails as a ZIP error in `oxoffice-opc`).

**Save**, the docx rule: staged in a synced sibling file and renamed over `path` by `oxoffice_opc::write_atomic_file`
(a failed save leaves the file as it was; a symbolic link kept, its target replaced; Unix mode bits kept). A workbook
opened from a file refuses to be saved over it, whatever the spelling of `path` (`Error::OverwritesSource`), unless
`SaveOptions { overwrite_source: true, .. }`; `drop_cached_values` is the other option (section 3). `.xlsx`, `.xlsm`,
`.xltx` or `.xltm` selects the main content type; a VBA project cannot be saved under a macro-free extension. Python
`save(path, *, overwrite_source=False, drop_cached_values=False)`, `to_bytes(*, drop_cached_values=False)`, the
encrypted saves with the same keyword; the CLI writes `-o`, never the input, `--force` replacing another file. Python
errors: `XlsxError`, `PackageError(XlsxError)`, `XmlError(XlsxError, ValueError)`, a refused edit `XlsxError`, a bad
argument the built-in `ValueError`, `TypeError`, `IndexError` or `KeyError`.

### 9. Producers

The model never branches on the producer; these are the shapes its tolerant read and its tests must cover.

| Topic | Excel (*to check on the corpus*) | LibreOffice 26.2 (probed) | Google Sheets (probed) |
| --- | --- | --- | --- |
| shared formulas | filled columns as groups | none: groups expanded into plain formulas | groups, `si` from 1 |
| booleans | `t="b"` | `t="b"` with `<f>TRUE()</f>` / `<f>FALSE()</f>`: read as formula cells | `t="b"` |
| numbers | shortest text | shortest, `t="n"` written | constants `1.0`, formula results `3` |
| attribute booleans | `1` / `0` | mostly `true` / `false` | `1` / `0` |
| rows | `spans`, `x14ac:dyDescent` | every row attribute written | empty `<row ht customHeight/>` up to row 1000, no `dimension` |
| recalculation | `calcChain.xml`, `calcId` | no chain, no `fullCalcOnLoad` | `<calcPr/>` empty |
| dynamic arrays | `cm` + `metadata.xml` | metadata dropped, a legacy array written | *to check* |
| strings | shared; rich runs | shared, inline strings converted | shared, no `xml:space` |
| comments | threads + `tc=` note copies + `persons` | notes with VML | an empty `personList` part even without comments; threads *to check* (`google/threads.xlsx`) |
| styles | complete `cellXfs` | `numFmtId 164` redefined as `General`, codes respelled (`[RED]`, `\-`), `<xf></xf>` with end tags, `*unknown*` style name | a font without `sz`, fill 1 `lightGray` |
| other | `xr:uid`, `mc:Ignorable`, `x14ac` | `<workbookProtection/>` empty, `docProps/custom.xml` added | no `docProps`, an empty drawing, `<definedNames/>`, `mx`/`mv`/`x14`/`x15`/`x14ac`/`xm` declared |

### 10. Blocks for the render of milestone 2

The render follows the docx and pptx path: model -> `oxoffice-xlsx-layout` (milestone 2) -> `oxoffice-layout` output
-> `oxoffice-pdf`. This set builds what that crate needs, so it adds itself without a remodel:

- `oxoffice-numfmt` (displayed text, colour, `_` / `*` hints, PR 4), `ResolvedStyle` with its colours resolved (PR 5)
  and `PageSetup` (typed in PR 3, read in PR 5): read now, painted later.
- `SheetGeometry` in `oxoffice-xlsx`, defined here, filled in the second set: the `Normal` font's maximum digit width
  in pixels (measured by the layout crate and passed in, so the facade needs no layout dependency); the default
  column width (`defaultColWidth`, else `baseColWidth` plus padding) and row height; column spans (width in
  characters, hidden, custom, outline level); rows (height in points, hidden, custom); merges; drawing anchors
  (two-cell, one-cell, absolute); the frozen pane. It gives each column's width and x and each row's height and y in
  points, characters going to pixels by ECMA-376 Part 1, 18.3.1.13 (`col`), pixels to points at 96 dpi.
- Existing charts render through `oxoffice-chart` from their cached values; nothing to build now.

### Not in this set

The evaluator; sheet rename, remove, reorder, insertion at a position; row and column insertion; writing comments,
names, tables, validations, conditional formats, hidden rows, filters, merges, theme colours; per-column CSV types;
document properties; the render; WebAssembly bindings.

## Plan

Each pull request carries the Rust facade, the CLI and Python, lean tests (table-driven, one driver each, Rust for the
semantics, Python for the binding layer only) and its `docs/` updates.

| PR | Content | Checked by |
| --- | --- | --- |
| 3 `feat(xlsx): open and save workbooks without loss` | `oxoffice-xlsx-oxml`, `oxoffice-xlsx`, `-cli`, `-py`; refusals (section 8); typed parts, row index and splice writer; `PageSetup`; atomic save; Agile encryption open and save; `inspect`; Python `Workbook`, `sheets`, `save`, `to_bytes` | every corpus `.xlsx` opens, saves without an edit and reopens with every ZIP entry byte for byte (the docx corpus test); a second walk reads every typed part and every row, writes it through its writer (a test hook) and checks both that reading it again gives an equal model and that its canonical form (`oxoffice-xml-test`, SpreadsheetML spellings folded: `true`/`1`, `false`/`0`, `1.0`/`1`, shortest double) equals the source's, which catches children out of schema order; the xlsx parts PR 3 writes join the equivalent-writings walk of `oxoffice-xml-test` (a root binding `x:`, a default namespace, end tags, self-closed elements); one row per refusal and per compound file kind, the files built in the test; encrypted round trip |
| 5 `feat(xlsx): read cells, formulas, styles and comments` | `oxoffice-xlsx-formula` (lexer, stored and displayed forms, mover, syntax check); cells, stale rule, shared formulas, resolved style (`apply_hls_tint`), displayed text, merges, comments, page setup; `text`, `--json`, `--markdown`, `csv`, `cells`, `comments`; bulk read | one table: for each family, every producer's copy reads the same values, formulas and displayed texts as the authored source at listed cells, producer differences named in the table; reading then writing every corpus formula gives back its stored text; the syntax check accepts every corpus formula and refuses a table of bad ones (`;` separators, a dangling or doubled operator, unbalanced brackets, a wrong argument count, 256 arguments, 65 levels); a mover table; `_xHHHH_` read and written in a table (a carriage return, a control character, a literal `_x0041_`); resolved styles of `budget`; comments of the two `threads` files |
| 6 `feat(xlsx): edit cells, formulas and styles` | section 6, recalculating save, edit map, `import-csv` | a table per rule (a 1D and a 2D group dissolved, refusals with their messages, style and string reuse, missing containers created, spill blocks in a file with and without `metadata.xml`, chain removal, `drop_cached_values` with and without an edit); a corpus walk editing one cell per file: exactly the expected parts change; the main checks: on the Mac, Excel opens every edited copy (each also carrying a style edit that appends a font, a fill, a border and an `xf`) without a repair prompt and recalculates the expected values, and Google Sheets imports them with the same values (orchestrator); LibreOffice headless opens every edited copy without an error and, saved with `drop_cached_values`, reads the expected value of a dependent total |
| 7 `feat(xlsx): new workbook, sheets, widths, heights, frozen panes` | section 7 | two builds of the new workbook give the same bytes; LibreOffice and, on the Mac, Excel open the new and edited workbooks without a repair prompt; a table per operation |

Docs: PR 3 adds the xlsx family to `docs/ARCHITECTURE.md` (crates, the dependency rule, the flow), the README format
table and `CLAUDE.md`'s first line; PR 4 adds `oxoffice-numfmt` there; PR 5 adds README read examples (Rust, CLI,
Python); PR 6 and 7 the edit and creation examples. Known limitations follow the repo rule (one file per case, with
its issue).

**Expected from PR 2 (corpus)**: `review`, `budget` and `multi` authored and saved by the three producers,
`excel/threads.xlsx`, `google/threads.xlsx`, a 1904 workbook, and manifest features (shared and array formulas, notes,
threads, merges, tables, validation, conditional formats, panes, print setup, hidden sheet, 1904, rich text, inline
strings, picture, chart, hyperlink) that choose the files each test walks.

**Expected from PR 4 (`oxoffice-numfmt`)**: `format(code, value, options)` for numbers and text: the text, the section
colour (a name or a palette index the workbook resolves through its `indexedColors`), the `_` / `*` positions; options
with the date system (en-US output); `builtin_code(id)` and whether an id is locale-independent (safe to reuse on
write); `is_date_format(code)`; serial to date-time and back in both systems, serial 60 of 1900 included.

## Decided with the spec

1. **Strict workbooks** are read and kept (section 8): pull request 2 adds `excel/strict.xlsx` (`review` saved as
   "Strict Open XML Spreadsheet" on the Mac).
2. **Formulas that need Excel 365 metadata** are written: a spilling formula on request (section 6), LET and LAMBDA
   with `_xlpm.` on their parameters' declarations and uses (section 2), so no limitation is needed.
3. **0-based `(row, column)` positions in Python** next to 1-based A1 strings (section 4): kept, the rule of both
   packages; openpyxl users read the A1 form.
4. **The new workbook's font**: Calibri 11 under the Office 2013-2022 theme, not Excel 365's Aptos Narrow: Carlito
   bundles its metrics for the render, and LibreOffice and Google substitute it alike.
5. **Excel checks on the Mac**, each a fact this spec assumes: the `ca="1"` mark on volatile formulas; a written
   formula without `<v>` opens without a repair prompt; a font record without `sz`; the `rgb` alpha byte; theme
   indexes 2 and 3 (`lt2`, `dk2`, swapped like 0 and 1); the `tc=` note copy of a thread. The corpus copies answer
   what they can; the PR 6 and PR 7 copies go through one Mac session for the rest.

## Links

- [#365](https://github.com/oxoffice-dev/oxoffice/issues/365) and its plan comment; tensorbee/rdocx issue 281;
  [#466](https://github.com/oxoffice-dev/oxoffice/issues/466).
- [Number format codes](2026-11-27-number-format-codes.md) (PR 4);
  [One rule for every Python length argument](2026-11-21-one-rule-for-python-length-arguments.md);
  [Corpus at scale](2026-11-21-corpus-at-scale-and-behaviour-tests.md);
  [Equivalent XML writings](2026-11-25-equivalent-xml-writings.md);
  [Word parts with a default namespace](2026-11-26-word-parts-with-a-default-namespace.md);
  [Characters XML cannot carry](2026-11-22-characters-xml-cannot-carry.md).
