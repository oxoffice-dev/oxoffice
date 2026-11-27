"""Merges what Excel displayed for one round into tests/cases.tsv.

Reads from the round directory:
- cases-rN.tsv, written by cases.py;
- excel-rN.tsv: id and Excel's `string value` (Range.Text) of column C, one row per case, a header line;
- excel-csv-rN-1900.csv and excel-csv-rN-1904.csv: the workbooks saved as CSV by Excel, in UTF-8;
- optionally excel-csv-rN-<system>-lossy.txt: the ids of the rows whose CSV text lost characters (a CSV Excel
  saves in Mac Roman writes `_` for the others); those rows take Range.Text. Lines starting with `#` are notes.

The `excel` column takes Range.Text, or the CSV text where they differ: Range.Text depends on the column
width for fills, accounting formats and `#####`, the CSV does not. A cell full of `#` is written `#####`.
A case already in the table keeps its `ours` and `why`; a new case is added at the end.

Run: python3 -I pin.py <round-dir> <round>
"""

import csv
import pathlib
import sys

TABLE = pathlib.Path(__file__).resolve().parent.parent / "cases.tsv"
COLUMNS = ["id", "code", "value", "date_system", "kind", "excel", "ours", "why"]


def rows(path):
    return [line.rstrip("\n").split("\t") for line in open(path, encoding="utf-8")][1:]


def lossy_ids(paths):
    return {
        line.strip()
        for path in paths
        for line in open(path, encoding="utf-8")
        if line.strip() and not line.startswith("#")
    }


def csv_texts(paths):
    texts = {}
    for path in paths:
        with open(path, newline="", encoding="utf-8") as f:
            for row in list(csv.reader(f))[1:]:
                texts[row[0]] = row[2]
    return texts


def main(directory, number):
    cases = rows(directory / f"cases-r{number}.tsv")
    shown = dict(rows(directory / f"excel-r{number}.tsv"))
    exported = csv_texts(sorted(directory.glob(f"excel-csv-r{number}-*.csv")))
    lossy = lossy_ids(sorted(directory.glob(f"excel-csv-r{number}-*-lossy.txt")))
    table = {row[0]: row for row in rows(TABLE)} if TABLE.exists() else {}
    for case_id, code, value, system, kind in cases:
        excel = shown[case_id] if shown[case_id] == exported[case_id] or case_id in lossy else exported[case_id]
        if len(excel) >= 5 and set(excel) == {"#"}:
            excel = "#####"
        ours, why = table[case_id][6:8] if case_id in table else ("", "")
        fields = [case_id, code, value, system, kind, excel, ours, why]
        assert all("\t" not in field and "\n" not in field for field in fields), fields
        table[case_id] = fields
    lines = ["\t".join(COLUMNS)] + ["\t".join(row) for row in table.values()]
    TABLE.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(f"round {number}: {len(cases)} cases pinned, {len(table)} in {TABLE}")


if __name__ == "__main__":
    main(pathlib.Path(sys.argv[1]), int(sys.argv[2]))
