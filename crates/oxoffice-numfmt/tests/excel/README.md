# The Excel case table

`../cases.tsv` holds every case the number format test checks, with the text Excel displayed for it. The
cases are grouped in rounds, each checked once in Excel (round 1: 526 cases, round 2: 71, round 3: 24, round 4:
484, round 5: 241, round 6: 1326, round 7: 60, round 8: 63, in Excel for Mac 16.115, en-US).

A round:

1. Add the cases at the end of `cases.py`, after `ROUND = N`.
2. Build the round's workbooks: `uv run --with xlsxwriter python -I cases.py <dir> N`. It writes
   `cases-rN.tsv`, `cases-rN-1900.xlsx` and, when the round has 1904 cases, `cases-rN-1904.xlsx`. Excel keeps
   only the first 215 custom number formats of a workbook (the others read General), so a round with more than
   200 distinct codes is written in parts, `cases-rN-1900-p2.xlsx` and so on; the TSV of the answers is one.
3. In Excel launched in en-US, open each workbook and, for every row, read the `string value` of column C
   (AppleScript; `Range.Text` in VBA): write them to `<dir>/excel-rN.tsv`, `id<TAB>displayed` with a header
   line. Then save each workbook as CSV, in UTF-8 (transcode a plain CSV save), as
   `<dir>/excel-csv-rN-1900.csv` and `<dir>/excel-csv-rN-1904.csv` (one per part). Rows Excel could not show (a workbook that held too many formats)
   come back from another copy as `<dir>/excel-rN-<name>.tsv`, which `pin.py` takes over the main list. A CSV saved in Mac Roman writes `_` for a
   character outside it (Spanish `a. m.` with its narrow no-break space, CJK): list those ids in
   `<dir>/excel-csv-rN-1900-lossy.txt` (one per line, `#` for a note) and `pin.py` takes their text from
   `excel-rN.tsv`.
4. Pin the round: `python3 -I pin.py <dir> N`. The `excel` column takes the string value, or the CSV text
   where they differ (fills, accounting formats and `#####` depend on the column width); a cell full of `#`
   is written `#####`. Cases already in the table keep their `ours` and `why`.
5. A round that asks for month names, day names or the day period of a language (`[$-411]mmm"|"mmmm"|"mmmmm`,
   `ddd"|"dddd`, `h AM/PM`, as rounds 4 and 6 write them): add the language to `LANGUAGES` in `names.py`
   (the table each pinned tag is, the regions and text tags that read as it) and run `python3 -I names.py`: it
   writes `src/names/tables.rs` from the pinned rows and stops on a tag whose rows differ from its table's or that
   no table claims.
6. Run `cargo test -p oxoffice-numfmt`. A case where the crate differs from Excel on purpose gets its text in
   `ours` and the reason in `why`; any other difference is a defect to fix.
