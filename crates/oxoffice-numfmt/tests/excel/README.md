# The Excel case table

`../cases.tsv` holds every case the number format test checks, with the text Excel displayed for it. The
cases are grouped in rounds, each checked once in Excel (round 1: 528 cases, round 2: 71, in Excel for Mac
16.115, en-US).

A round:

1. Add the cases at the end of `cases.py`, after `ROUND = N`.
2. Build the round's workbooks: `uv run --with xlsxwriter python -I cases.py <dir> N`. It writes
   `cases-rN.tsv`, `cases-rN-1900.xlsx` and, when the round has 1904 cases, `cases-rN-1904.xlsx`.
3. In Excel launched in en-US, open each workbook and, for every row, read the `string value` of column C
   (AppleScript; `Range.Text` in VBA): write them to `<dir>/excel-rN.tsv`, `id<TAB>displayed` with a header
   line. Then save each workbook as CSV, in UTF-8 (transcode a plain CSV save), as
   `<dir>/excel-csv-rN-1900.csv` and `<dir>/excel-csv-rN-1904.csv`.
4. Pin the round: `python3 -I pin.py <dir> N`. The `excel` column takes the string value, or the CSV text
   where they differ (fills, accounting formats and `#####` depend on the column width); a cell full of `#`
   is written `#####`. Cases already in the table keep their `ours` and `why`.
5. Run `cargo test -p oxoffice-numfmt`. A case where the crate differs from Excel on purpose gets its text in
   `ours` and the reason in `why`; any other difference is a defect to fix.
