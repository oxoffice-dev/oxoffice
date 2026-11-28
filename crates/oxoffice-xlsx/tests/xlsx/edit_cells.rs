//! Cells written: values, dates, formulas, clears, bulk writes, styles and
//! CSV imports reach the rows they name as the producers write cells, a
//! shared formula group written into is dissolved, what Excel would repair
//! is refused with nothing changed, the shared strings, styles and
//! `metadata.xml` are reused, appended to or created, reads see the edits,
//! and a walk edits a cell of every workbook of the corpus, checking that
//! exactly the expected parts change.

use std::sync::Arc;

use oxoffice_numfmt::DateTime;
use oxoffice_xlsx::{
    CellError, CellInput, CellRange, Clear, CsvImport, DateValue, Error, FormulaOptions,
    SaveOptions, Spill, StylePatch, Value, Workbook, WorksheetMut,
};
use oxoffice_xlsx_oxml::styles::{StyleList, StylesXml};

use crate::support::{
    corpus_bytes, corpus_workbooks, holds_formula, kept_bytes, map_in_parallel, package_parts,
    part_text, parts_diff, workbook_with_sheet, workbook_with_sheets,
};

const MAIN: &str = r#"xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main""#;
const XR: &str = r#"xmlns:xr="http://schemas.microsoft.com/office/spreadsheetml/2014/revision""#;

fn sheet(data: &str) -> String {
    format!("<worksheet {MAIN} {XR}>{data}</worksheet>")
}

/// What a table row does to a sheet.
enum Op {
    Value(&'static str, CellInput),
    Formula(&'static str, &'static str, FormulaOptions),
    Clear(&'static str, Clear),
    Write(&'static str, Vec<Vec<CellInput>>),
    Style(&'static str, StylePatch),
    NumberFormat(&'static str, &'static str),
    Csv(&'static str, &'static [u8], bool),
}

fn apply(sheet: &mut WorksheetMut<'_>, ops: Vec<Op>) -> Result<(), Error> {
    for op in ops {
        match op {
            Op::Value(cell, input) => sheet.set_value(cell, input)?,
            Op::Formula(cell, text, options) => sheet.set_formula(cell, text, options)?,
            Op::Clear(range, what) => sheet.clear(range, what)?,
            Op::Write(cell, rows) => sheet.write(cell, rows)?,
            Op::Style(range, patch) => sheet.set_style(range, &patch)?,
            Op::NumberFormat(range, code) => sheet.set_number_format(range, code)?,
            Op::Csv(cell, csv, infer) => sheet.import_csv(cell, csv, CsvImport { infer })?,
        }
    }
    Ok(())
}

fn number(n: f64) -> CellInput {
    CellInput::Value(Value::Number(n))
}

fn text(t: &str) -> CellInput {
    CellInput::Value(Value::Text(t.to_owned()))
}

fn date(year: i32, month: u32, day: u32) -> CellInput {
    CellInput::Date(DateValue::Date { year, month, day })
}

const PLAIN: FormulaOptions = FormulaOptions {
    stored: false,
    spill: None,
};
const STORED: FormulaOptions = FormulaOptions {
    stored: true,
    spill: None,
};
const SPILL: FormulaOptions = FormulaOptions {
    stored: false,
    spill: Some(Spill::Anchor),
};

/// A formula that spills over `range`.
fn spill_over(range: &str) -> FormulaOptions {
    FormulaOptions {
        stored: false,
        spill: Some(Spill::Range(CellRange::parse(range).unwrap())),
    }
}

/// The sheet, the shared strings and the styles of `saved`.
fn parts(saved: &[u8]) -> (String, Option<String>, String) {
    let parts = package_parts(saved);
    let text = |name: &str| {
        parts
            .get(name)
            .map(|bytes| String::from_utf8(bytes.clone()).unwrap())
    };
    (
        text("xl/worksheets/sheet1.xml").unwrap(),
        text("xl/sharedStrings.xml"),
        text("xl/styles.xml").unwrap(),
    )
}

#[test]
fn each_value_is_written_where_the_sheet_says_it_is_as_excel_writes_it() {
    let cases: Vec<(&str, String, Vec<Op>, String)> = vec![
        (
            "a number into an empty sheet: its row and cell made, the dimension that said A1 replaced",
            sheet(r#"<dimension ref="A1"/><sheetData/>"#),
            vec![Op::Value("B2", number(1.5))],
            sheet(r#"<dimension ref="B2"/><sheetData><row r="2"><c r="B2"><v>1.5</v></c></row></sheetData>"#),
        ),
        (
            "each kind over cells a producer wrote: style and other attributes kept, type and value metadata gone, spans widened",
            sheet(concat!(
                r#"<dimension ref="A1:C1"/><sheetData><row r="1" spans="1:3">"#,
                r#"<c r="A1" s="0" xr:uid="{1}" t="s"><v>0</v></c>"#,
                r#"<c r="B1" t="inlineStr"><is><t>old</t></is></c>"#,
                r#"<c r="C1" t="e" vm="1"><v>#VALUE!</v><extLst><ext uri="x"/></extLst></c>"#,
                r#"</row></sheetData>"#
            )),
            vec![
                Op::Value("A1", CellInput::from(true)),
                Op::Value("B1", CellInput::from(CellError::NotAvailable)),
                Op::Value("C1", number(-0.0)),
                Op::Value("E1", number(1e21)),
            ],
            sheet(concat!(
                r#"<dimension ref="A1:E1"/><sheetData><row r="1" spans="1:5">"#,
                r#"<c r="A1" s="0" xr:uid="{1}" t="b"><v>1</v></c>"#,
                r#"<c r="B1" t="e"><v>#N/A</v></c>"#,
                r#"<c r="C1"><v>0</v><extLst><ext uri="x"/></extLst></c>"#,
                r#"<c r="E1"><v>1E+21</v></c>"#,
                r#"</row></sheetData>"#
            )),
        ),
        (
            "nothing written clears: a bare cell goes, a styled one keeps its style, a row with no cell stays",
            sheet(r#"<sheetData><row r="3"><c r="A3"><v>1</v></c><c r="B3" s="0"><f>A3</f><v>1</v></c></row></sheetData>"#),
            vec![
                Op::Value("A3", CellInput::Value(Value::Empty)),
                Op::Value("B3", text("")),
                Op::Value("C3", CellInput::Value(Value::Empty)),
            ],
            sheet(r#"<sheetData><row r="3"><c r="B3" s="0"/></row></sheetData>"#),
        ),
        (
            "a cell between cells, rows before, between and after, under the prefix the root binds",
            r#"<x:worksheet xmlns:x="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><x:sheetData><x:row r="2"><x:c r="A2"><x:v>1</x:v></x:c><x:c r="C2"><x:v>3</x:v></x:c></x:row><x:row r="4"/></x:sheetData></x:worksheet>"#.to_owned(),
            vec![
                Op::Value("B2", number(2.0)),
                Op::Value("A1", number(0.5)),
                Op::Value("A3", number(3.0)),
                Op::Value("A9", number(9.0)),
            ],
            r#"<x:worksheet xmlns:x="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><x:sheetData><x:row r="1"><x:c r="A1"><x:v>0.5</x:v></x:c></x:row><x:row r="2"><x:c r="A2"><x:v>1</x:v></x:c><x:c r="B2"><x:v>2</x:v></x:c><x:c r="C2"><x:v>3</x:v></x:c></x:row><x:row r="3"><x:c r="A3"><x:v>3</x:v></x:c></x:row><x:row r="4"/><x:row r="9"><x:c r="A9"><x:v>9</x:v></x:c></x:row></x:sheetData></x:worksheet>"#.to_owned(),
        ),
        (
            "a new cell takes the style of its row, else of its column, as it read",
            sheet(r#"<cols><col min="2" max="2" width="9" style="0"/><col min="3" max="3" width="9" style="7"/></cols><sheetData><row r="1" s="5" customFormat="1"/></sheetData>"#),
            vec![Op::Value("A1", number(1.0)), Op::Value("C2", number(2.0))],
            sheet(r#"<cols><col min="2" max="2" width="9" style="0"/><col min="3" max="3" width="9" style="7"/></cols><sheetData><row r="1" s="5" customFormat="1"><c r="A1" s="5"><v>1</v></c></row><row r="2"><c r="C2" s="7"><v>2</v></c></row></sheetData>"#),
        ),
        (
            "a bulk write, a ragged row leaving the cells after it, the last write of a cell winning",
            sheet(r#"<sheetData><row r="2"><c r="C2"><v>9</v></c></row></sheetData>"#),
            vec![
                Op::Write("A1", vec![vec![number(1.0), number(2.0), number(3.0)], vec![number(4.0)]]),
                Op::Write("B1", vec![vec![CellInput::from(false)]]),
            ],
            sheet(r#"<sheetData><row r="1"><c r="A1"><v>1</v></c><c r="B1" t="b"><v>0</v></c><c r="C1"><v>3</v></c></row><row r="2"><c r="A2"><v>4</v></c><c r="C2"><v>9</v></c></row></sheetData>"#),
        ),
        (
            "a clear of contents, of formats and of both",
            sheet(r#"<sheetData><row r="1"><c r="A1" s="0"><v>1</v></c><c r="B1" s="0"><v>2</v></c><c r="C1" s="0"><v>3</v></c></row></sheetData>"#),
            vec![
                Op::Clear("A1", Clear::Contents),
                Op::Clear("B1:B5", Clear::Formats),
                Op::Clear("C1:D9", Clear::All),
            ],
            sheet(r#"<sheetData><row r="1"><c r="A1" s="0"/><c r="B1"><v>2</v></c></row></sheetData>"#),
        ),
        (
            "a CSV from a cell: typed fields, an empty field and a short line clearing their cells",
            sheet(r#"<sheetData><row r="2"><c r="B2"><v>7</v></c><c r="C2"><v>8</v></c></row></sheetData>"#),
            vec![Op::Csv("B1", b"1,TRUE\r\n,007\r\n-2.5\r\n", true)],
            sheet(r#"<sheetData><row r="1"><c r="B1"><v>1</v></c><c r="C1" t="b"><v>1</v></c></row><row r="2"><c r="C2" t="s"><v>0</v></c></row><row r="3"><c r="B3"><v>-2.5</v></c></row></sheetData>"#),
        ),
    ];
    for (case, source, ops, expected) in cases {
        let mut workbook = workbook_with_sheet(&source);
        apply(&mut workbook.sheet_at_mut(0).unwrap(), ops)
            .unwrap_or_else(|e| panic!("{case}: {e}"));
        let saved = workbook.to_bytes().unwrap();
        assert_eq!(
            part_text(&saved, "xl/worksheets/sheet1.xml"),
            expected,
            "{case}"
        );
        // What was written reads back, and written again says the same.
        let again = Workbook::from_bytes(&saved).unwrap();
        assert_eq!(again.to_bytes().unwrap(), saved, "{case}");
    }
}

#[test]
fn texts_go_to_the_shared_strings_reusing_a_plain_item_never_a_rich_one() {
    // A workbook without shared strings gets the part, its relationship
    // and its content type; a text written twice is one item.
    let mut workbook = workbook_with_sheet(&sheet("<sheetData/>"));
    let mut data = workbook.sheet_at_mut(0).unwrap();
    data.set_value("A1", "Total").unwrap();
    data.set_value("A2", "Total").unwrap();
    data.set_value("A3", "line\r\nnext _x0041_\u{1}").unwrap();
    let saved = workbook.to_bytes().unwrap();
    let (xml, strings, _) = parts(&saved);
    assert!(xml.contains(r#"<c r="A1" t="s"><v>0</v></c>"#), "{xml}");
    assert!(xml.contains(r#"<c r="A2" t="s"><v>0</v></c>"#), "{xml}");
    assert!(
        strings.unwrap().ends_with(
            r#"<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="3" uniqueCount="2"><si><t>Total</t></si><si><t>line_x000D_
next _x005F_x0041__x0001_</t></si></sst>"#
        )
    );
    let content_types = part_text(&saved, "[Content_Types].xml");
    assert!(content_types.contains(r#"PartName="/xl/sharedStrings.xml""#));
    assert!(part_text(&saved, "xl/_rels/workbook.xml.rels").contains("sharedStrings.xml"));
    let reread = Workbook::from_bytes(&saved).unwrap();
    let sheet = reread.sheet_at(0).unwrap();
    assert_eq!(
        sheet.cell("A3").unwrap().value,
        Value::Text("line\r\nnext _x0041_\u{1}".to_owned())
    );

    // An existing part: a plain item is reused, a rich item with the same
    // text is not, and `count` follows the cells that point at an item.
    let mut review = Workbook::from_bytes(&corpus_bytes("excel/review.xlsx")).unwrap();
    let strings_before = part_text(&corpus_bytes("excel/review.xlsx"), "xl/sharedStrings.xml");
    let mut orders = review.sheet_at_mut(0).unwrap();
    orders.set_value("A2", "Status").unwrap();
    orders.set_value("H1", "Reviewer").unwrap();
    orders.set_value("G2", 3.0).unwrap();
    let saved = review.to_bytes().unwrap();
    let strings = part_text(&saved, "xl/sharedStrings.xml");
    let count = |xml: &str, name: &str| -> u32 {
        let at = xml.find(&format!(" {name}=\"")).unwrap() + name.len() + 3;
        xml[at..].split('"').next().unwrap().parse().unwrap()
    };
    assert_eq!(
        count(&strings, "uniqueCount"),
        count(&strings_before, "uniqueCount") + 1
    );
    assert_eq!(
        count(&strings, "count"),
        count(&strings_before, "count") + 1 - 1 + 1 - 1
    );
    assert!(
        strings.ends_with("<si><t>Reviewer</t></si></sst>"),
        "{strings}"
    );
    let reread = Workbook::from_bytes(&saved).unwrap();
    let orders = reread.sheet_at(0).unwrap();
    assert_eq!(orders.cell("A2").unwrap().text, "Status");
    assert_eq!(orders.cell("H1").unwrap().text, "Reviewer");
}

#[test]
fn a_date_writes_its_serial_and_a_cell_shown_as_general_a_format_that_shows_it() {
    let at = |hour, minute, second| DateTime {
        year: 2026,
        month: 1,
        day: 5,
        hour,
        minute,
        second,
        millisecond: 0,
    };
    let cases: Vec<(CellInput, f64, &str, &str)> = vec![
        (date(2026, 1, 5), 46027.0, "yyyy-mm-dd", "2026-01-05"),
        (
            CellInput::Date(DateValue::DateTime(at(18, 0, 0))),
            46027.75,
            "yyyy-mm-dd hh:mm:ss",
            "2026-01-05 18:00:00",
        ),
        (
            CellInput::Date(DateValue::Time {
                hour: 6,
                minute: 0,
                second: 0,
                millisecond: 0,
            }),
            0.25,
            "hh:mm:ss",
            "06:00:00",
        ),
        (
            CellInput::Date(DateValue::Duration(93_600.0)),
            1.083_333_333_333_333_3,
            "[h]:mm:ss",
            "26:00:00",
        ),
    ];
    let mut workbook = workbook_with_sheet(&sheet("<sheetData/>"));
    for (row, (input, serial, code, shown)) in (1..).zip(cases) {
        let cell = format!("A{row}");
        let mut data = workbook.sheet_at_mut(0).unwrap();
        data.set_value(&cell, input).unwrap();
        let read = workbook.sheet_at(0).unwrap().cell(&cell).unwrap();
        assert!(
            matches!(read.value, Value::Number(n) if (n - serial).abs() < 1e-9),
            "{cell}: {:?}",
            read.value
        );
        assert_eq!(
            (read.number_format.as_str(), read.text.as_str()),
            (code, shown),
            "{cell}"
        );
    }
    let read = workbook.sheet_at(0).unwrap().cell("A4").unwrap();
    assert_eq!(read.duration, Some(std::time::Duration::from_secs(93_600)));
    // A formatted cell keeps its format; the same date again reuses the
    // records.
    let mut data = workbook.sheet_at_mut(0).unwrap();
    data.set_value("A1", date(2027, 3, 1)).unwrap();
    data.set_value("B1", date(2027, 3, 1)).unwrap();
    data.set_number_format("C1", "d mmm").unwrap();
    data.set_value("C1", date(2027, 3, 1)).unwrap();
    let sheet = workbook.sheet_at(0).unwrap();
    assert_eq!(sheet.cell("C1").unwrap().text, "1 Mar");
    assert_eq!(
        sheet.cell("A1").unwrap().style,
        sheet.cell("B1").unwrap().style
    );
    let saved = workbook.to_bytes().unwrap();
    let styles =
        StylesXml::parse(Arc::new(package_parts(&saved)["xl/styles.xml"].clone())).unwrap();
    // Four codes and the built-in [h]:mm:ss (id 46, which French Excel keeps),
    // one cell format each, after the first.
    assert_eq!(styles.list(StyleList::NumFmts).len(), 4);
    assert!(styles.cell_xfs().any(|xf| xf.num_fmt_id() == 46));
    assert_eq!(styles.list(StyleList::CellXfs).len(), 6);
    // In the 1904 system, the serial of that system.
    let mut d1904 = Workbook::from_bytes(&corpus_bytes("excel/dates1904.xlsx")).unwrap();
    d1904
        .sheet_at_mut(0)
        .unwrap()
        .set_value("H1", date(2026, 1, 5))
        .unwrap();
    assert_eq!(
        d1904.sheet_at(0).unwrap().cell("H1").unwrap().value,
        Value::Number(44565.0)
    );
}

/// The stored formula and attributes of the `f` of `cell` in `xml`.
fn formula_of(xml: &str, cell: &str) -> String {
    let start = xml
        .find(&format!(r#"<c r="{cell}""#))
        .unwrap_or_else(|| panic!("{cell} in {xml}"));
    let end = start
        + xml[start..]
            .find("</c>")
            .map_or_else(|| xml[start..].find("/>").unwrap() + 2, |at| at + 4);
    xml[start..end].to_owned()
}

#[test]
fn a_formula_is_written_in_its_stored_form_once_checked() {
    let cases: Vec<(&str, FormulaOptions, &str)> = vec![
        ("=D2*E2", PLAIN, r#"<c r="A1"><f>D2*E2</f></c>"#),
        (
            "XLOOKUP(A2,Rates!$A$2:$A$9,Rates!$B$2:$B$9)",
            PLAIN,
            r#"<c r="A1"><f>_xlfn.XLOOKUP(A2,Rates!$A$2:$A$9,Rates!$B$2:$B$9)</f></c>"#,
        ),
        (
            "LET(f,LAMBDA(x,[y],x*2),f(3))",
            PLAIN,
            r#"<c r="A1"><f>_xlfn.LET(_xlpm.f,_xlfn.LAMBDA(_xlpm.x,_xlop.y,_xlpm.x*2),_xlpm.f(3))</f></c>"#,
        ),
        (
            "GROUPBY(A2:A9,B2:B9,SUM)",
            PLAIN,
            r#"<c r="A1"><f>_xlfn.GROUPBY(A2:A9,B2:B9,_xleta.SUM)</f></c>"#,
        ),
        ("@A20:A22", PLAIN, r#"<c r="A1"><f>A20:A22</f></c>"#),
        ("@MyName", PLAIN, r#"<c r="A1"><f>MyName</f></c>"#),
        (
            "@INDEX(A20:A22,2)",
            PLAIN,
            r#"<c r="A1"><f>_xlfn.SINGLE(INDEX(A20:A22,2))</f></c>"#,
        ),
        (
            "@(A20+B20)",
            PLAIN,
            r#"<c r="A1"><f>_xlfn.SINGLE((A20+B20))</f></c>"#,
        ),
        (
            "_xlfn.XLOOKUP(1,B:B,C:C)",
            STORED,
            r#"<c r="A1"><f>_xlfn.XLOOKUP(1,B:B,C:C)</f></c>"#,
        ),
        (
            "SORT(B2:B7,,-1)",
            SPILL,
            r#"<c r="A1" cm="1"><f t="array" ref="A1">_xlfn._xlws.SORT(B2:B7,,-1)</f></c>"#,
        ),
        (
            "@A20:A22",
            SPILL,
            r#"<c r="A1" cm="1"><f t="array" ref="A1">_xlfn.SINGLE(A20:A22)</f></c>"#,
        ),
        (
            "SORT(B2:B7,,-1)",
            spill_over("A1:A6"),
            r#"<c r="A1" cm="1"><f t="array" ref="A1:A6">_xlfn._xlws.SORT(B2:B7,,-1)</f></c>"#,
        ),
    ];
    for (input, options, expected) in cases {
        let mut workbook = workbook_with_sheet(&sheet("<sheetData/>"));
        workbook
            .sheet_at_mut(0)
            .unwrap()
            .set_formula("A1", input, options)
            .unwrap_or_else(|e| panic!("{input}: {e}"));
        let saved = workbook.to_bytes().unwrap();
        let (xml, _, _) = parts(&saved);
        assert_eq!(formula_of(&xml, "A1"), expected, "{input}");
        // A workbook without a formula until now is calculated on open.
        assert!(
            part_text(&saved, "xl/workbook.xml").contains(r#"fullCalcOnLoad="1""#),
            "{input}"
        );
        let read = Workbook::from_bytes(&saved).unwrap();
        let cell = read.sheet_at(0).unwrap().cell("A1").unwrap();
        assert!(cell.stale && cell.value == Value::Empty, "{input}");
    }
    // What Excel would repair is refused with a plain message, and nothing
    // changes.
    for (input, options, message) in [
        ("IF(B2>0;1;0)", PLAIN, "write `,` between arguments"),
        ("B2*", PLAIN, "not a formula Excel reads"),
        ("B2**C2", PLAIN, "not a formula Excel reads"),
        ("IF(1,2,3,4)", PLAIN, "not a formula Excel reads"),
        (
            "XLOOKUP(1,B:B,C:C)",
            STORED,
            "Excel stores this formula as `_xlfn.XLOOKUP(1,B:B,C:C)`",
        ),
    ] {
        let mut workbook = workbook_with_sheet(&sheet("<sheetData/>"));
        let before = workbook.to_bytes().unwrap();
        let error = workbook
            .sheet_at_mut(0)
            .unwrap()
            .set_formula("A1", input, options)
            .unwrap_err();
        assert!(
            matches!(error, Error::InvalidArgument(_)),
            "{input}: {error:?}"
        );
        assert!(error.to_string().contains(message), "{input}: {error}");
        assert_eq!(workbook.to_bytes().unwrap(), before, "{input}");
    }
}

#[test]
fn a_spilling_formula_finds_or_appends_its_metadata_and_makes_the_part_when_missing() {
    // Excel's multi.xlsx has the dynamic array block: found, the part kept.
    let original = corpus_bytes("excel/multi.xlsx");
    let mut multi = Workbook::from_bytes(&original).unwrap();
    multi
        .sheet_at_mut(0)
        .unwrap()
        .set_formula("J2", "UNIQUE(A4:A7)", SPILL)
        .unwrap();
    let saved = multi.to_bytes().unwrap();
    assert_eq!(
        package_parts(&saved)["xl/metadata.xml"],
        package_parts(&original)["xl/metadata.xml"]
    );
    assert!(
        formula_of(&part_text(&saved, "xl/worksheets/sheet1.xml"), "J2")
            .starts_with(r#"<c r="J2" cm="1"><f t="array" ref="J2">_xlfn.UNIQUE(A4:A7)</f>"#)
    );
    let read = Workbook::from_bytes(&saved).unwrap();
    let cell = read.sheet_at(0).unwrap().cell("J2").unwrap();
    assert_eq!(
        cell.array_range.map(|range| range.to_string()).as_deref(),
        Some("J2")
    );

    // LibreOffice drops it: the part is made, with its relationship and
    // content type, as Excel writes it.
    let mut lo = Workbook::from_bytes(&corpus_bytes("libreoffice/multi.xlsx")).unwrap();
    lo.sheet_at_mut(0)
        .unwrap()
        .set_formula("J2", "UNIQUE(A4:A7)", SPILL)
        .unwrap();
    lo.sheet_at_mut(0)
        .unwrap()
        .set_formula("K2", "SORT(A4:A7)", SPILL)
        .unwrap();
    let saved = lo.to_bytes().unwrap();
    let metadata = part_text(&saved, "xl/metadata.xml");
    assert_eq!(metadata, part_text(&original, "xl/metadata.xml"));
    assert!(part_text(&saved, "[Content_Types].xml").contains(r#"<Override PartName="/xl/metadata.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheetMetadata+xml"/>"#));
    assert!(
        part_text(&saved, "xl/_rels/workbook.xml.rels")
            .contains("relationships/sheetMetadata\" Target=\"metadata.xml\"")
    );
    assert!(
        formula_of(&part_text(&saved, "xl/worksheets/sheet1.xml"), "K2")
            .starts_with(r#"<c r="K2" cm="1">"#)
    );

    // A range given: written as the ref, so that Google Sheets spills it too,
    // and read as the array's range. It must start at the cell and be free.
    let mut workbook = workbook_with_sheet(&sheet(concat!(
        r#"<sheetData><row r="1"><c r="B1"><f t="array" ref="B1:B3">1</f><v>1</v></c><c r="D1"><v>4</v></c></row></sheetData>"#,
        r#"<mergeCells count="1"><mergeCell ref="F1:G2"/></mergeCells>"#
    )));
    let mut data = workbook.sheet_at_mut(0).unwrap();
    data.set_formula("H1", "SEQUENCE(3)", spill_over("H1:H3"))
        .unwrap();
    for (cell, range, message) in [
        ("H5", "H4:H9", "the range H4:H9 starts at H4, not at H5"),
        ("C1", "C1:D2", "D1 holds a value or a formula: clear it"),
        ("A2", "A2:B2", "B1:B3 holds an array formula"),
        ("E2", "E2:F2", "F1:G2 is merged"),
    ] {
        let error = data
            .set_formula(cell, "SEQUENCE(2)", spill_over(range))
            .unwrap_err();
        assert!(
            error.to_string().contains(message),
            "{cell} {range}: {error}"
        );
    }
    let sheet_read = workbook.sheet_at(0).unwrap();
    for cell in ["H1", "H3"] {
        let read = sheet_read.cell(cell).unwrap();
        assert_eq!(
            read.array_range.map(|range| range.to_string()).as_deref(),
            Some("H1:H3"),
            "{cell}"
        );
    }
}

#[test]
fn a_write_into_a_shared_formula_group_turns_its_other_cells_into_plain_formulas() {
    let cases: Vec<(&str, String, Op, String)> = vec![
        (
            "a member of a column: the master and every other member get their text, values kept",
            sheet(concat!(
                r#"<sheetData><row r="2"><c r="C2"><f t="shared" ref="C2:C4" si="0">A2*B2</f><v>2</v></c></row>"#,
                r#"<row r="3"><c r="C3"><f t="shared" si="0"/><v>6</v></c></row>"#,
                r#"<row r="4"><c r="C4"><f t="shared" si="0" ca="1"/><v>12</v></c></row></sheetData>"#
            )),
            Op::Value("C3", number(5.0)),
            sheet(concat!(
                r#"<sheetData><row r="2"><c r="C2"><f>A2*B2</f><v>2</v></c></row>"#,
                r#"<row r="3"><c r="C3"><v>5</v></c></row>"#,
                r#"<row r="4"><c r="C4"><f ca="1">A4*B4</f><v>12</v></c></row></sheetData>"#
            )),
        ),
        (
            "the master of a block: each member moved in both directions, $ parts kept",
            sheet(concat!(
                r#"<sheetData><row r="1"><c r="B1"><f t="shared" ref="B1:C2" si="3">$A1+A$1</f><v>1</v></c><c r="C1"><f t="shared" si="3"/><v>1</v></c></row>"#,
                r#"<row r="2"><c r="B2"><f t="shared" si="3"/><v>1</v></c><c r="C2"><f t="shared" si="3"/><v>1</v></c></row></sheetData>"#
            )),
            Op::Formula("B1", "1", PLAIN),
            sheet(concat!(
                r#"<sheetData><row r="1"><c r="B1"><f>1</f></c><c r="C1"><f>$A1+B$1</f><v>1</v></c></row>"#,
                r#"<row r="2"><c r="B2"><f>$A2+A$1</f><v>1</v></c><c r="C2"><f>$A2+B$1</f><v>1</v></c></row></sheetData>"#
            )),
        ),
        (
            "a style alone dissolves nothing",
            sheet(
                r#"<sheetData><row r="1"><c r="A1"><f t="shared" ref="A1:A2" si="0">1</f><v>1</v></c></row><row r="2"><c r="A2"><f t="shared" si="0"/><v>1</v></c></row></sheetData>"#,
            ),
            Op::Style(
                "A2",
                StylePatch {
                    bold: Some(true),
                    ..StylePatch::default()
                },
            ),
            sheet(
                r#"<sheetData><row r="1"><c r="A1"><f t="shared" ref="A1:A2" si="0">1</f><v>1</v></c></row><row r="2"><c r="A2" s="1"><f t="shared" si="0"/><v>1</v></c></row></sheetData>"#,
            ),
        ),
    ];
    for (case, source, op, expected) in cases {
        let mut workbook = workbook_with_sheet(&source);
        apply(&mut workbook.sheet_at_mut(0).unwrap(), vec![op])
            .unwrap_or_else(|e| panic!("{case}: {e}"));
        let saved = workbook.to_bytes().unwrap();
        assert_eq!(
            part_text(&saved, "xl/worksheets/sheet1.xml"),
            expected,
            "{case}"
        );
    }

    // The corpus column of `review`: every member keeps the formula it read.
    for copy in ["authored", "excel", "google"] {
        let original = corpus_bytes(&format!("{copy}/review.xlsx"));
        let before = Workbook::from_bytes(&original).unwrap();
        let formulas: Vec<Option<String>> = (2..=37)
            .map(|row| {
                before
                    .sheet_at(0)
                    .unwrap()
                    .cell(&format!("F{row}"))
                    .unwrap()
                    .formula
            })
            .collect();
        let mut workbook = Workbook::from_bytes(&original).unwrap();
        workbook
            .sheet_at_mut(0)
            .unwrap()
            .set_value("F10", 1.0)
            .unwrap();
        let saved = Workbook::from_bytes(&workbook.to_bytes().unwrap()).unwrap();
        for (row, formula) in (2..=37).zip(formulas) {
            let cell = saved.sheet_at(0).unwrap().cell(&format!("F{row}")).unwrap();
            if row == 10 {
                assert_eq!(
                    (cell.formula, cell.value),
                    (None, Value::Number(1.0)),
                    "{copy}"
                );
            } else {
                assert_eq!(cell.formula, formula, "{copy}: F{row}");
                assert_eq!(cell.formula_kind, None, "{copy}: F{row}");
            }
        }
    }
}

#[test]
fn what_excel_would_repair_or_this_version_does_not_write_is_refused_and_changes_nothing() {
    let pivot_and_array = sheet(concat!(
        r#"<sheetData><row r="1"><c r="A1"><f t="array" ref="A1:A2">B1:B2</f><v>1</v></c><c r="C1"><f t="dataTable" ref="C1:D1" dt2D="0" r1="E1"/><v>1</v></c></row>"#,
        r#"<row r="2"><c r="A2"><v>2</v></c></row></sheetData><mergeCells count="1"><mergeCell ref="F1:G2"/></mergeCells>"#
    ));
    let cases: Vec<(&str, Op, &str)> = vec![
        (
            "part of an array formula",
            Op::Value("A2", number(1.0)),
            "A1:A2 holds an array formula whose cells are written together",
        ),
        (
            "a clear of part of one",
            Op::Clear("A2:B2", Clear::Contents),
            "write or clear all of A1:A2",
        ),
        (
            "a data table",
            Op::Formula("D1", "1", PLAIN),
            "C1:D1 holds a data table",
        ),
        (
            "a spill into an array",
            Op::Formula("A2", "SORT(B1:B2)", SPILL),
            "A1:A2 holds an array formula",
        ),
        (
            "a value under a merge",
            Op::Value("G2", number(1.0)),
            "G2 is covered by the merged range F1:G2: its value goes in F1",
        ),
        (
            "a cell XFE1",
            Op::Value("XFE1", number(1.0)),
            "`XFE1` is not a cell or a range",
        ),
        (
            "a range for a cell",
            Op::Value("A1:B2", number(1.0)),
            "`A1:B2` is not a cell",
        ),
        (
            "a number that is not one",
            Op::Value("B5", number(f64::NAN)),
            "a finite number, not NaN",
        ),
        (
            "a text too long",
            Op::Value("B5", text(&"x".repeat(32_768))),
            "at most 32,767 characters",
        ),
        (
            "an error of a later Excel",
            Op::Value(
                "B5",
                CellInput::from(CellError::Other("#SPILL!".to_owned())),
            ),
            "#SPILL! is not an error value a cell stores",
        ),
        (
            "a date the calendar lacks",
            Op::Value("B5", date(2026, 2, 30)),
            "2026-02-30 is not a date",
        ),
        (
            "a negative duration",
            Op::Value("B5", CellInput::Date(DateValue::Duration(-1.0))),
            "a duration is a number of seconds from 0",
        ),
        (
            "a font size",
            Op::Style(
                "B5",
                StylePatch {
                    font_size: Some(0.5),
                    ..StylePatch::default()
                },
            ),
            "a font size is from 1 to 409",
        ),
        (
            "a colour",
            Op::Style(
                "B5",
                StylePatch {
                    fill: Some("red".to_owned()),
                    ..StylePatch::default()
                },
            ),
            "fill is a colour of six hex digits",
        ),
        (
            "a border style",
            Op::Style(
                "B5",
                StylePatch {
                    border: Some("wavy".to_owned()),
                    ..StylePatch::default()
                },
            ),
            "\"wavy\" is not a border style",
        ),
        (
            "a border colour alone",
            Op::Style(
                "B5",
                StylePatch {
                    border_color: Some("FF0000".to_owned()),
                    ..StylePatch::default()
                },
            ),
            "border_color goes with border",
        ),
        (
            "an alignment",
            Op::Style(
                "B5",
                StylePatch {
                    horizontal: Some("middle".to_owned()),
                    ..StylePatch::default()
                },
            ),
            "\"middle\" is not a horizontal alignment",
        ),
        (
            "a format code",
            Op::NumberFormat("B5", "[Red"),
            "is not a number format code Excel reads",
        ),
        (
            "a style over too many cells",
            Op::NumberFormat("A1:Z50000", "0"),
            "at most 1000000 are styled at once",
        ),
        (
            "a CSV not in UTF-8",
            Op::Csv("A5", b"caf\xe9", true),
            "byte 3 (0xE9)",
        ),
    ];
    for (case, op, message) in cases {
        let mut workbook = workbook_with_sheet(&pivot_and_array);
        let before = workbook.to_bytes().unwrap();
        let error = apply(&mut workbook.sheet_at_mut(0).unwrap(), vec![op]).unwrap_err();
        assert!(error.to_string().contains(message), "{case}: {error}");
        assert_eq!(workbook.to_bytes().unwrap(), before, "{case}");
    }
    // A whole array is written and cleared; a merged range's first cell and
    // its other cells' styles are written.
    let mut workbook = workbook_with_sheet(&pivot_and_array);
    let mut data = workbook.sheet_at_mut(0).unwrap();
    data.write("A1", vec![vec![number(1.0)], vec![number(2.0)]])
        .unwrap();
    data.clear("C1:D1", Clear::Contents).unwrap();
    data.set_value("F1", "merged").unwrap();
    // Nothing written over a cell that holds nothing changes nothing, and is
    // not refused.
    data.set_value("G1", CellInput::Value(Value::Empty))
        .unwrap();
    data.set_style(
        "G2",
        &StylePatch {
            bold: Some(true),
            ..StylePatch::default()
        },
    )
    .unwrap();
    let sheet_after = workbook.sheet_at(0).unwrap();
    assert_eq!(sheet_after.cell("A2").unwrap().formula_kind, None);
    assert_eq!(sheet_after.cell("C1").unwrap().formula_kind, None);

    // A table's header, written as it is or not at all; a pivot table's
    // cells, not at all.
    for copy in ["excel", "google", "libreoffice"] {
        let mut budget =
            Workbook::from_bytes(&corpus_bytes(&format!("{copy}/budget.xlsx"))).unwrap();
        let before = budget.to_bytes().unwrap();
        let mut ledger = budget.sheet_mut("Ledger").unwrap();
        for (cell, input) in [
            ("F4", text("Total")),
            ("F4", number(1.0)),
            ("B4", CellInput::Value(Value::Empty)),
        ] {
            let error = ledger.set_value(cell, input).unwrap_err();
            assert!(matches!(error, Error::Refused(_)), "{copy}");
            assert!(
                error
                    .to_string()
                    .contains("of table Expenses: a new header renames the column"),
                "{copy}: {error}"
            );
        }
        assert_eq!(budget.to_bytes().unwrap(), before, "{copy}");
        budget
            .sheet_mut("Ledger")
            .unwrap()
            .set_value("F4", "Amount")
            .unwrap();
        budget
            .sheet_mut("Ledger")
            .unwrap()
            .set_value("F5", 1.0)
            .unwrap();
    }
    let mut package = oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(
        workbook_with_sheet(&sheet("<sheetData/>"))
            .to_bytes()
            .unwrap(),
    ))
    .unwrap();
    package.set_part("/xl/pivotTables/pivotTable1.xml", br#"<pivotTableDefinition xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" name="Sales by region"><location ref="A3:C9" firstHeaderRow="1" firstDataRow="1" firstDataCol="1"/></pivotTableDefinition>"#.to_vec());
    package.content_types.add_override(
        "/xl/pivotTables/pivotTable1.xml",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.pivotTable+xml",
    );
    package
        .get_or_create_part_rels("/xl/worksheets/sheet1.xml")
        .add(
            "http://schemas.openxmlformats.org/officeDocument/2006/relationships/pivotTable",
            "../pivotTables/pivotTable1.xml",
        );
    let mut bytes = std::io::Cursor::new(Vec::new());
    package.write_to(&mut bytes).unwrap();
    let mut pivot = Workbook::from_bytes(bytes.get_ref()).unwrap();
    let error = pivot
        .sheet_at_mut(0)
        .unwrap()
        .set_value("B4", 1.0)
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("B4 is in pivot table Sales by region (A3:C9)"),
        "{error}"
    );
    pivot.sheet_at_mut(0).unwrap().set_value("D4", 1.0).unwrap();
}

#[test]
fn a_style_edit_reuses_identical_records_and_appends_what_is_missing() {
    let patch = StylePatch {
        bold: Some(true),
        font_color: Some("#1f4e79".to_owned()),
        fill: Some("FFFF00".to_owned()),
        border: Some("thin".to_owned()),
        horizontal: Some("center".to_owned()),
        wrap_text: Some(true),
        number_format: Some("#,##0.00".to_owned()),
        ..StylePatch::default()
    };
    let mut workbook = workbook_with_sheet(&sheet("<sheetData/>"));
    let mut data = workbook.sheet_at_mut(0).unwrap();
    data.set_style("A1:B2", &patch).unwrap();
    data.set_style("C3", &patch).unwrap();
    let saved = workbook.to_bytes().unwrap();
    let (xml, _, styles) = parts(&saved);
    // One font, fill, border and cell format appended for the five cells.
    assert!(
        xml.contains(r#"<row r="1"><c r="A1" s="1"/><c r="B1" s="1"/></row>"#),
        "{xml}"
    );
    assert!(xml.contains(r#"<c r="C3" s="1"/>"#), "{xml}");
    assert!(styles.contains(r#"<fonts count="2"><font><sz val="11"/><color theme="1"/><name val="Calibri"/><family val="2"/><scheme val="minor"/></font><font><b/><sz val="11"/><color rgb="FF1F4E79"/><name val="Calibri"/><family val="2"/><scheme val="minor"/></font></fonts>"#), "{styles}");
    assert!(styles.contains(r#"<fill><patternFill patternType="solid"><fgColor rgb="FFFFFF00"/><bgColor indexed="64"/></patternFill></fill></fills>"#), "{styles}");
    assert!(styles.contains(r#"<border><left style="thin"><color indexed="64"/></left><right style="thin"><color indexed="64"/></right><top style="thin"><color indexed="64"/></top><bottom style="thin"><color indexed="64"/></bottom><diagonal/></border></borders>"#), "{styles}");
    assert!(styles.contains(r#"<cellXfs count="2"><xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/><xf numFmtId="4" fontId="1" fillId="2" borderId="1" xfId="0" applyFont="1" applyFill="1" applyBorder="1" applyAlignment="1" applyNumberFormat="1"><alignment horizontal="center" wrapText="1"/></xf></cellXfs>"#), "{styles}");
    let read = Workbook::from_bytes(&saved).unwrap();
    let style = read.sheet_at(0).unwrap().cell("B2").unwrap().style;
    assert!(style.font.bold && style.font.color.as_ref().unwrap().rgb.as_deref() == Some("1F4E79"));
    assert_eq!(style.number_format, "#,##0.00");

    // A patch that says what a cell's style says already changes nothing.
    let mut plain = workbook_with_sheet(&sheet(
        r#"<sheetData><row r="1"><c r="A1"><v>1</v></c></row></sheetData>"#,
    ));
    let untouched = plain.to_bytes().unwrap();
    let patch = StylePatch {
        bold: Some(false),
        number_format: Some("General".to_owned()),
        ..StylePatch::default()
    };
    plain
        .sheet_at_mut(0)
        .unwrap()
        .set_style("A1", &patch)
        .unwrap();
    assert_eq!(plain.to_bytes().unwrap(), untouched);

    // Number formats: a built-in id every language shows alike, a custom
    // code reused, a new id after the largest (LibreOffice's 164 is
    // General), a date's built-in id never reused.
    let mut lo = Workbook::from_bytes(&corpus_bytes("libreoffice/review.xlsx")).unwrap();
    let styles_before = StylesXml::parse(Arc::new(
        package_parts(&corpus_bytes("libreoffice/review.xlsx"))["xl/styles.xml"].clone(),
    ))
    .unwrap();
    let mut orders = lo.sheet_at_mut(0).unwrap();
    for (cell, code) in [
        ("H1", "0.00%"),
        ("H2", "#,##0.00"),
        ("H3", "0.000"),
        ("H4", "m/d/yy"),
        ("H5", "0.000"),
    ] {
        orders.set_number_format(cell, code).unwrap();
    }
    let saved = lo.to_bytes().unwrap();
    let styles =
        StylesXml::parse(Arc::new(package_parts(&saved)["xl/styles.xml"].clone())).unwrap();
    let added: Vec<(Option<u32>, String)> = styles
        .num_fmts()
        .skip(styles_before.num_fmts().count())
        .map(|format| (format.id(), format.code()))
        .collect();
    let largest = styles_before
        .num_fmts()
        .filter_map(|format| format.id())
        .max()
        .unwrap();
    assert_eq!(
        added,
        [
            (Some(largest + 1), "0.000".to_owned()),
            (Some(largest + 2), "m/d/yy".to_owned())
        ]
    );
    let read = Workbook::from_bytes(&saved).unwrap();
    let orders = read.sheet_at(0).unwrap();
    let id = |cell: &str| {
        let s = orders.cell(cell).unwrap();
        s.number_format
    };
    assert_eq!(id("H1"), "0.00%");
    assert_eq!(id("H2"), "#,##0.00");

    // A styles part without numFmts (Google's when no cell has a custom
    // format, the new workbook's): the list is made at its place.
    let mut new = workbook_with_sheet(&sheet("<sheetData/>"));
    new.sheet_at_mut(0)
        .unwrap()
        .set_number_format("Z1", "0.000")
        .unwrap();
    let styles = part_text(&new.to_bytes().unwrap(), "xl/styles.xml");
    assert!(styles.contains(r#"<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><numFmts count="1"><numFmt numFmtId="164" formatCode="0.000"/></numFmts><fonts"#), "{styles}");
}

#[test]
fn whole_columns_and_rows_take_the_style_themselves_so_cells_typed_later_take_it() {
    let bold = || StylePatch {
        bold: Some(true),
        ..StylePatch::default()
    };
    let italic = StylePatch {
        italic: Some(true),
        ..StylePatch::default()
    };
    let cases: Vec<(&str, String, Vec<Op>, String)> = vec![
        (
            "columns: a span cut keeps its width, a column without one gets the default width, their cells and a styled row's cells take the style",
            sheet(
                r#"<cols><col min="2" max="3" width="12" customWidth="1"/></cols><sheetData><row r="1"><c r="A1"><v>1</v></c><c r="C1"><v>3</v></c></row><row r="2" s="0" customFormat="1"/></sheetData>"#,
            ),
            vec![Op::Style("C:D", bold())],
            sheet(concat!(
                r#"<cols><col min="2" max="2" width="12" customWidth="1"/><col min="3" max="3" width="12" customWidth="1" style="1"/><col min="4" max="4" width="9.140625" style="1"/></cols>"#,
                r#"<sheetData><row r="1"><c r="A1"><v>1</v></c><c r="C1" s="1"><v>3</v></c></row><row r="2" s="0" customFormat="1"><c r="C2" s="1"/><c r="D2" s="1"/></row></sheetData>"#
            )),
        ),
        (
            "rows: the row's style and its cells', a styled column's cell holding both, a row the sheet lacks made",
            sheet(r#"<sheetData><row r="2"><c r="A2"><v>1</v></c></row></sheetData>"#),
            vec![Op::Style("C:C", italic.clone()), Op::Style("2:3", bold())],
            sheet(concat!(
                r#"<cols><col min="3" max="3" width="9.140625" style="1"/></cols><sheetData>"#,
                r#"<row r="2" s="2" customFormat="1"><c r="A2" s="2"><v>1</v></c><c r="C2" s="3"/></row>"#,
                r#"<row r="3" s="2" customFormat="1"><c r="C3" s="3"/></row></sheetData>"#
            )),
        ),
        (
            "a number format of columns, then their formats cleared: the spans and cells lose the style",
            sheet(r#"<sheetData><row r="1"><c r="B1"><v>1</v></c></row></sheetData>"#),
            vec![
                Op::NumberFormat("B:C", "0.00"),
                Op::Clear("B:B", Clear::Formats),
            ],
            sheet(
                r#"<cols><col min="2" max="2" width="9.140625"/><col min="3" max="3" width="9.140625" style="1"/></cols><sheetData><row r="1"><c r="B1"><v>1</v></c></row></sheetData>"#,
            ),
        ),
        (
            "rows cleared of everything: contents, the row's style and its cells'",
            sheet(r#"<sheetData><row r="1"><c r="A1"><v>1</v></c></row></sheetData>"#),
            vec![Op::Style("1:1", bold()), Op::Clear("1:1", Clear::All)],
            sheet(r#"<sheetData><row r="1"><c r="A1"/></row></sheetData>"#),
        ),
    ];
    for (case, source, ops, expected) in cases {
        let mut workbook = workbook_with_sheet(&source);
        apply(&mut workbook.sheet_at_mut(0).unwrap(), ops)
            .unwrap_or_else(|e| panic!("{case}: {e}"));
        let saved = workbook.to_bytes().unwrap();
        assert_eq!(
            part_text(&saved, "xl/worksheets/sheet1.xml"),
            expected,
            "{case}"
        );
    }

    // A cell typed later in a styled column or row takes the style, and so
    // does a cell the sheet does not write.
    let mut workbook = workbook_with_sheet(&sheet("<sheetData/>"));
    let mut data = workbook.sheet_at_mut(0).unwrap();
    data.set_style("B:B", &bold()).unwrap();
    data.set_style("5:5", &italic).unwrap();
    data.set_value("B9", 1.0).unwrap();
    let sheet_read = workbook.sheet_at(0).unwrap();
    for (cell, bold, italic) in [
        ("B9", true, false),
        ("B100", true, false),
        ("C5", false, true),
        ("B5", true, true),
        ("A1", false, false),
    ] {
        let font = sheet_read.cell(cell).unwrap().style.font.clone();
        assert_eq!((font.bold, font.italic), (bold, italic), "{cell}");
    }
    let error = workbook
        .sheet_at_mut(0)
        .unwrap()
        .set_style("1:200000", &bold())
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("at most 100000 are styled at once"),
        "{error}"
    );
}

#[test]
fn reads_after_edits_see_the_staged_rows() {
    let mut workbook = Workbook::from_bytes(&corpus_bytes("excel/multi.xlsx")).unwrap();
    let used_before = workbook.sheet_at(0).unwrap().used_range().unwrap().unwrap();
    let mut summary = workbook.sheet_at_mut(0).unwrap();
    summary.set_value("A4", "Northeast").unwrap();
    summary.set_formula("Z40", "SUM(B4:B7)", PLAIN).unwrap();
    // F2:F7 is a spilled SORT: written whole, its range goes.
    summary
        .write("F2", (0..6).map(|n| vec![number(f64::from(n))]).collect())
        .unwrap();
    let sheet = workbook.sheet_at(0).unwrap();
    assert_eq!(
        sheet.cell("A4").unwrap().value,
        Value::Text("Northeast".to_owned())
    );
    assert_eq!(
        sheet.cell("Z40").unwrap().formula.as_deref(),
        Some("SUM(B4:B7)")
    );
    assert!(
        sheet.cell("B4").unwrap().stale,
        "a formula's cached value is stale after an edit"
    );
    assert_eq!(sheet.cell("F3").unwrap().array_range, None);
    assert_eq!(
        sheet.values("F2:F3").unwrap(),
        [[Value::Number(0.0)], [Value::Number(1.0)]]
    );
    assert_eq!(sheet.cells("A4:A4").unwrap()[0].text, "Northeast");
    let used = sheet.used_range().unwrap().unwrap();
    assert_eq!(
        (used.first, used.last.row, used.last.column),
        (used_before.first, 39, 25)
    );
    assert!(workbook.recalculation_needed());
}

#[test]
fn drop_cached_values_leaves_every_formula_to_calculate_with_or_without_an_edit() {
    let source = sheet(concat!(
        r#"<sheetData><row r="1"><c r="A1"><v>2</v></c><c r="B1" t="str"><f>A1&amp;"x"</f><v>2x</v></c>"#,
        r#"<c r="C1" cm="1" vm="1" t="e"><f t="array" ref="C1:C2">A1:A2</f><v>#VALUE!</v></c></row>"#,
        r#"<row r="2"><c r="C2"><v>3</v></c></row><row r="3"><c r="A3"><v>3</v></c></row></sheetData>"#
    ));
    let dropped = sheet(concat!(
        r#"<sheetData><row r="1"><c r="A1"><v>2</v></c><c r="B1"><f>A1&amp;&quot;x&quot;</f></c>"#,
        r#"<c r="C1" cm="1"><f t="array" ref="C1:C2">A1:A2</f></c></row>"#,
        r#"<row r="2"><c r="C2"/></row><row r="3"><c r="A3"><v>3</v></c></row></sheetData>"#
    ));
    let options = SaveOptions {
        drop_cached_values: true,
        ..SaveOptions::default()
    };
    for edit in [false, true] {
        let mut workbook = workbook_with_sheets(&[&source], true);
        if edit {
            workbook
                .sheet_at_mut(0)
                .unwrap()
                .set_row_height("9", 20.0)
                .unwrap();
        }
        let saved = workbook.to_bytes_with_options(options).unwrap();
        let xml = part_text(&saved, "xl/worksheets/sheet1.xml");
        let expected = if edit {
            dropped.replace(
                "</sheetData>",
                r#"<row r="9" ht="20" customHeight="1"/></sheetData>"#,
            )
        } else {
            dropped.clone()
        };
        assert_eq!(xml, expected, "edit {edit}");
        assert!(part_text(&saved, "xl/workbook.xml").contains(r#"fullCalcOnLoad="1""#));
        assert!(!package_parts(&saved).contains_key("xl/calcChain.xml"));
        // The workbook itself keeps its values.
        assert_eq!(
            workbook.sheet_at(0).unwrap().cell("B1").unwrap().value,
            Value::Text("2x".to_owned())
        );
    }
    // Without a formula, nothing to drop: saved as it was.
    let plain = workbook_with_sheet(&sheet(
        r#"<sheetData><row r="1"><c r="A1"><v>1</v></c></row></sheetData>"#,
    ));
    assert_eq!(
        plain.to_bytes_with_options(options).unwrap(),
        plain.to_bytes().unwrap()
    );
}

/// The cell each corpus family has a value feeding a formula in: the sheet,
/// the cell, the cell whose formula reads it.
fn feeding(path: &str) -> (&'static str, &'static str) {
    match path.rsplit('/').next().unwrap() {
        "budget.xlsx" => ("Budget", "B4"),
        "dates1904.xlsx" => ("", "B2"),
        "multi.xlsx" => ("Sales", "C2"),
        "stream.xlsx" => ("", "E2"),
        _ => ("", "D2"),
    }
}

/// Writes a number into the cell a formula reads, in the sheet `feeding`
/// names, and checks what changed.
fn edited_cell(original: &[u8], path: &str) -> Result<(), String> {
    let mut workbook = Workbook::from_bytes(original).map_err(|e| e.to_string())?;
    let (name, cell) = feeding(path);
    let position = if name.is_empty() {
        0
    } else {
        workbook.sheet(name).ok_or("no sheet")?.position()
    };
    let relationship = workbook
        .sheet_at(position)
        .unwrap()
        .relationship_id()
        .to_owned();
    workbook
        .sheet_at_mut(position)
        .unwrap()
        .set_value(cell, 7.0)
        .map_err(|e| e.to_string())?;
    let saved = workbook.to_bytes().map_err(|e| e.to_string())?;

    let package = oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(original)).unwrap();
    let target = &package
        .get_part_rels("/xl/workbook.xml")
        .unwrap()
        .get_by_id(&relationship)
        .unwrap()
        .target;
    let sheet_part = oxoffice_opc::OpcPackage::resolve_rel_target("/xl/workbook.xml", target)
        .trim_start_matches('/')
        .to_owned();
    let before = package_parts(original);
    let formula = holds_formula(&before);
    let had_chain = before.contains_key("xl/calcChain.xml");
    let asks = part_text(original, "xl/workbook.xml").contains("fullCalcOnLoad=\"1\"");
    let mut expected = vec![sheet_part.clone()];
    if formula && !asks {
        expected.push("xl/workbook.xml".to_owned());
    }
    if formula && had_chain {
        expected.extend([
            "[Content_Types].xml".to_owned(),
            "xl/_rels/workbook.xml.rels".to_owned(),
        ]);
    }
    expected.sort();
    let diff = parts_diff(original, &saved);
    let mut changed = diff.changed.clone();
    changed.sort();
    if changed != expected || !diff.added.is_empty() {
        return Err(format!(
            "changed {changed:?}, added {:?}, expected {expected:?}",
            diff.added
        ));
    }
    let after = package_parts(&saved);
    let row = oxoffice_xlsx::CellRef::parse(cell).unwrap().row;
    if kept_bytes(&before[&sheet_part], &["dimension"], &[row])
        != kept_bytes(&after[&sheet_part], &["dimension"], &[row])
    {
        return Err("a byte outside the edited row changed".to_owned());
    }
    let reread = Workbook::from_bytes(&saved).map_err(|e| e.to_string())?;
    let read = reread
        .sheet_at(position)
        .unwrap()
        .cell(cell)
        .map_err(|e| e.to_string())?;
    if read.value != Value::Number(7.0) {
        return Err(format!("{cell} reads {:?}", read.value));
    }
    Ok(())
}

#[test]
fn every_workbook_takes_a_value_in_a_cell_and_only_its_sheet_and_calculation_change() {
    let files = corpus_workbooks();
    let outcomes = map_in_parallel(&files, |file| edited_cell(&file.bytes(), &file.path));
    let failures: Vec<String> = files
        .iter()
        .zip(outcomes)
        .filter_map(
            |(file, outcome)| match outcome.unwrap_or_else(|| Err("panicked".to_owned())) {
                Ok(()) => None,
                Err(error) => Some(format!("{}: {error}", file.path)),
            },
        )
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
