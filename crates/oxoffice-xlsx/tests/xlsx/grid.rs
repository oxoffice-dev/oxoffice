//! Column widths, row heights and frozen panes reach the sheet and nothing
//! else: a table of sheets written as the producers write them, the refusals
//! with their messages, and a walk that edits the first sheet of every
//! workbook of the corpus and checks that exactly the expected parts and
//! bytes changed.

use oxoffice_xlsx::{Error, Workbook, WorksheetMut};
use oxoffice_xlsx_oxml::Node;
use oxoffice_xlsx_oxml::workbook::WorkbookXml;
use std::sync::Arc;

use crate::support::{
    child_of, corpus_workbooks, kept_bytes, map_in_parallel, package_parts, part_text, parts_diff,
    row_of, workbook_with_sheet,
};

const MAIN: &str = r#"xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main""#;

/// What a table row does to a sheet.
enum Op {
    Width(&'static str, f64),
    Height(&'static str, f64),
    Freeze(Option<&'static str>),
}

fn apply(sheet: &mut WorksheetMut<'_>, ops: &[Op]) -> Result<(), Error> {
    for op in ops {
        match op {
            Op::Width(columns, width) => sheet.set_column_width(columns, *width)?,
            Op::Height(rows, height) => sheet.set_row_height(rows, *height)?,
            Op::Freeze(cell) => sheet.freeze_panes(*cell)?,
        }
    }
    Ok(())
}

#[test]
fn each_edit_writes_its_element_at_its_place_in_the_sheet_as_the_sheet_writes_its_own() {
    let panes = |view: &str| {
        format!(
            r#"{view}<pane ySplit="1" topLeftCell="A2" activePane="bottomLeft" state="frozen"/><selection pane="bottomLeft" activeCell="A2" sqref="A2"/>"#
        )
    };
    let lo_row = |attributes: &str| {
        format!(
            r#"<row r="2" customFormat="false" {attributes} hidden="false" outlineLevel="0" collapsed="false"><c r="A2"><v>1</v></c></row>"#
        )
    };
    let cases: Vec<(&str, String, Vec<Op>, String)> = vec![
        (
            "a sheet with only its data gets cols before it",
            format!("<worksheet {MAIN}><sheetData/></worksheet>"),
            vec![Op::Width("A:B", 10.0)],
            format!(
                r#"<worksheet {MAIN}><cols><col min="1" max="2" width="10.7109375" customWidth="1"/></cols><sheetData/></worksheet>"#
            ),
        ),
        (
            "cols and the panes go where the schema puts them, among the children a producer wrote",
            format!(
                r#"<worksheet {MAIN}><sheetPr/><dimension ref="A1"/><sheetViews><sheetView workbookViewId="0"/></sheetViews><sheetFormatPr defaultRowHeight="15"/><sheetData><row r="1"><c r="A1"><v>1</v></c></row></sheetData><pageMargins left="0.7" right="0.7" top="0.75" bottom="0.75" header="0.3" footer="0.3"/></worksheet>"#
            ),
            vec![Op::Width("C", 20.0), Op::Freeze(Some("A2"))],
            format!(
                r#"<worksheet {MAIN}><sheetPr/><dimension ref="A1"/><sheetViews>{}</sheetView></sheetViews><sheetFormatPr defaultRowHeight="15"/><cols><col min="3" max="3" width="20.7109375" customWidth="1"/></cols><sheetData><row r="1"><c r="A1"><v>1</v></c></row></sheetData><pageMargins left="0.7" right="0.7" top="0.75" bottom="0.75" header="0.3" footer="0.3"/></worksheet>"#,
                panes(r#"<sheetView workbookViewId="0">"#)
            ),
        ),
        (
            "a sheet with no sheetViews gets them after its dimension",
            format!(
                r#"<worksheet {MAIN}><dimension ref="A1"/><sheetFormatPr defaultRowHeight="15"/><sheetData/></worksheet>"#
            ),
            vec![Op::Freeze(Some("A2"))],
            format!(
                r#"<worksheet {MAIN}><dimension ref="A1"/><sheetViews>{}</sheetView></sheetViews><sheetFormatPr defaultRowHeight="15"/><sheetData/></worksheet>"#,
                panes(r#"<sheetView workbookViewId="0">"#)
            ),
        ),
        (
            "a root that binds a prefix: every element written takes it",
            r#"<x:worksheet xmlns:x="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><x:sheetData><x:row r="2"><x:c r="A2"/></x:row></x:sheetData></x:worksheet>"#.to_owned(),
            vec![Op::Width("A", 5.0), Op::Height("2:3", 30.0), Op::Freeze(Some("A2"))],
            r#"<x:worksheet xmlns:x="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><x:sheetViews><x:sheetView workbookViewId="0"><x:pane ySplit="1" topLeftCell="A2" activePane="bottomLeft" state="frozen"/><x:selection pane="bottomLeft" activeCell="A2" sqref="A2"/></x:sheetView></x:sheetViews><x:cols><x:col min="1" max="1" width="5.7109375" customWidth="1"/></x:cols><x:sheetData><x:row r="2" ht="30" customHeight="1"><x:c r="A2"/></x:row><x:row r="3" ht="30" customHeight="1"/></x:sheetData></x:worksheet>"#.to_owned(),
        ),
        (
            "rows that are written: a row with cells keeps them and its other attributes, LibreOffice's spellings fixed",
            format!("<worksheet {MAIN}><sheetData>{}</sheetData></worksheet>", lo_row(r#"ht="12.8" customHeight="false""#)),
            vec![Op::Height("2", 30.0)],
            format!(
                r#"<worksheet {MAIN}><sheetData><row r="2" customFormat="0" ht="30" customHeight="1" hidden="0" outlineLevel="0" collapsed="0"><c r="A2"><v>1</v></c></row></sheetData></worksheet>"#
            ),
        ),
        (
            "rows that are not written are written between the rows around them, and after the last",
            format!(
                r#"<worksheet {MAIN}><sheetData><row r="1"/><row r="5"><c r="A5"/></row></sheetData></worksheet>"#
            ),
            vec![Op::Height("3", 20.25), Op::Height("7", 15.5)],
            format!(
                r#"<worksheet {MAIN}><sheetData><row r="1"/><row r="3" ht="20.25" customHeight="1"/><row r="5"><c r="A5"/></row><row r="7" ht="15.5" customHeight="1"/></sheetData></worksheet>"#
            ),
        ),
        (
            "rows written without their number keep it implied",
            format!(r#"<worksheet {MAIN}><sheetData><row><c><v>1</v></c></row><row/></sheetData></worksheet>"#),
            vec![Op::Height("2", 30.0)],
            format!(
                r#"<worksheet {MAIN}><sheetData><row><c><v>1</v></c></row><row ht="30" customHeight="1"/></sheetData></worksheet>"#
            ),
        ),
        (
            "a height over rows written and not, then widths over the same columns twice",
            format!(r#"<worksheet {MAIN}><sheetData/></worksheet>"#),
            vec![
                Op::Height("1:2", 30.0),
                Op::Width("B:C", 12.0),
                Op::Width("C:D", 14.0),
            ],
            format!(
                r#"<worksheet {MAIN}><cols><col min="2" max="2" width="12.7109375" customWidth="1"/><col min="3" max="4" width="14.7109375" customWidth="1"/></cols><sheetData><row r="1" ht="30" customHeight="1"/><row r="2" ht="30" customHeight="1"/></sheetData></worksheet>"#
            ),
        ),
        (
            "the panes removed, and the selections of the panes that went",
            format!(
                r#"<worksheet {MAIN}><sheetViews><sheetView workbookViewId="0"><pane xSplit="1" topLeftCell="B1" activePane="topRight" state="frozen"/><selection pane="topRight" activeCell="B1" sqref="B1"/></sheetView></sheetViews><sheetData/></worksheet>"#
            ),
            vec![Op::Freeze(None)],
            format!(
                r#"<worksheet {MAIN}><sheetViews><sheetView workbookViewId="0"/></sheetViews><sheetData/></worksheet>"#
            ),
        ),
    ];
    for (case, source, ops, expected) in cases {
        let mut workbook = workbook_with_sheet(&source);
        apply(&mut workbook.sheet_at_mut(0).unwrap(), &ops)
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
fn an_edit_that_changes_nothing_marks_nothing() {
    let source = format!(
        r#"<worksheet {MAIN}><sheetViews><sheetView workbookViewId="0"/></sheetViews><cols><col min="1" max="1" width="10.7109375" customWidth="1"/></cols><sheetData><row r="2" ht="30" customHeight="true"/></sheetData></worksheet>"#
    );
    let untouched = workbook_with_sheet(&source).to_bytes().unwrap();
    let mut workbook = workbook_with_sheet(&source);
    let mut sheet = workbook.sheet_at_mut(0).unwrap();
    sheet.set_column_width("A", 10.0).unwrap();
    sheet.set_row_height("2", 30.0).unwrap();
    sheet.freeze_panes(None).unwrap();
    assert_eq!(workbook.to_bytes().unwrap(), untouched);
}

#[test]
fn an_edit_that_cannot_be_made_is_refused_with_the_way_out_and_changes_nothing() {
    type Call = fn(&mut WorksheetMut<'_>) -> Result<(), Error>;
    let cases: Vec<(&str, Call, &str)> = vec![
        (
            "no columns",
            |s| s.set_column_width("", 5.0),
            "not a column or columns",
        ),
        (
            "rows as columns",
            |s| s.set_column_width("2", 5.0),
            "not a column or columns",
        ),
        (
            "a width of 0",
            |s| s.set_column_width("A", 0.0),
            "would hide the columns",
        ),
        (
            "a negative width",
            |s| s.set_column_width("A", -1.0),
            "from 0 to 255",
        ),
        (
            "a wide width",
            |s| s.set_column_width("A", 255.5),
            "from 0 to 255",
        ),
        (
            "a width that is no number",
            |s| s.set_column_width("A", f64::NAN),
            "from 0 to 255",
        ),
        (
            "no rows",
            |s| s.set_row_height("A", 5.0),
            "not a row or rows",
        ),
        ("row 0", |s| s.set_row_height("0", 5.0), "not a row or rows"),
        (
            "a height of 0",
            |s| s.set_row_height("1", 0.0),
            "would hide the rows",
        ),
        (
            "a tall height",
            |s| s.set_row_height("1", 409.6),
            "from 0 to 409.5",
        ),
        (
            "a span of the sheet",
            |s| s.set_row_height("1:1048576", 5.0),
            "at most 100000 are written",
        ),
        ("A1", |s| s.freeze_panes(Some("A1")), "no rows above it"),
        ("no cell", |s| s.freeze_panes(Some("")), "not a cell"),
        ("a range", |s| s.freeze_panes(Some("B2:C3")), "not a cell"),
        (
            "a cell outside the sheet",
            |s| s.freeze_panes(Some("XFE1")),
            "not a cell",
        ),
    ];
    let mut workbook = Workbook::new();
    let before = workbook.to_bytes().unwrap();
    for (case, call, message) in cases {
        let error = call(&mut workbook.sheet_at_mut(0).unwrap()).unwrap_err();
        assert!(
            matches!(error, Error::InvalidArgument(_) | Error::Refused(_)),
            "{case}"
        );
        assert!(error.to_string().contains(message), "{case}: {error}");
    }
    assert_eq!(workbook.to_bytes().unwrap(), before);

    // A sheet that holds no cells has no columns, rows or panes.
    let mut package = oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(before)).unwrap();
    package
        .get_part_rels_mut("/xl/workbook.xml")
        .unwrap()
        .items
        .iter_mut()
        .find(|relationship| relationship.target == "worksheets/sheet1.xml")
        .unwrap()
        .rel_type =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/chartsheet".to_owned();
    let mut bytes = std::io::Cursor::new(Vec::new());
    package.write_to(&mut bytes).unwrap();
    let mut chartsheet = Workbook::from_bytes(bytes.get_ref()).unwrap();
    let mut sheet = chartsheet.sheet_at_mut(0).unwrap();
    for error in [
        sheet.set_column_width("A", 5.0).unwrap_err(),
        sheet.set_row_height("1", 5.0).unwrap_err(),
        sheet.freeze_panes(Some("B2")).unwrap_err(),
    ] {
        assert!(matches!(error, Error::Refused(_)));
        assert!(error.to_string().contains("is a chartsheet"), "{error}");
    }
}

/// The ZIP entry of the sheet at `position`, by the relationship that names it.
fn part_of(bytes: &[u8], workbook: &Workbook, position: usize) -> String {
    let package = oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(bytes)).unwrap();
    let relationship = package
        .get_part_rels("/xl/workbook.xml")
        .and_then(|relationships| {
            relationships.get_by_id(workbook.sheet_at(position).unwrap().relationship_id())
        })
        .expect("the relationship of the sheet");
    oxoffice_opc::OpcPackage::resolve_rel_target("/xl/workbook.xml", &relationship.target)
        .trim_start_matches('/')
        .to_owned()
}

fn first_worksheet(workbook: &Workbook) -> usize {
    workbook
        .sheets()
        .find(|sheet| sheet.kind() == oxoffice_xlsx::SheetKind::Worksheet)
        .expect("a worksheet")
        .position()
}

const OPS: [Op; 3] = [
    Op::Width("B:C", 20.0),
    Op::Height("2:3", 30.0),
    Op::Freeze(Some("B2")),
];

/// Edits the first worksheet of `original` and checks what changed.
fn edited_sheet(original: &[u8]) -> Result<bool, String> {
    let mut workbook = Workbook::from_bytes(original).map_err(|e| e.to_string())?;
    let position = first_worksheet(&workbook);
    let sheet_part = part_of(original, &workbook, position);
    apply(&mut workbook.sheet_at_mut(position).unwrap(), &OPS).map_err(|e| e.to_string())?;
    let saved = workbook.to_bytes().map_err(|e| e.to_string())?;

    // The parts that changed: the sheet, the workbook (its calcPr), and the
    // chain a recalculating save drops with its relationship and type.
    let before = package_parts(original);
    let diff = parts_diff(original, &saved);
    let had_chain = before.contains_key("xl/calcChain.xml");
    // The workbook is written unless it asked for a calculation already, as
    // XlsxWriter's do.
    let asks = WorkbookXml::parse(Arc::new(before["xl/workbook.xml"].clone()))
        .map_err(|e| e.to_string())?
        .calc_pr()
        .is_some_and(|calc| calc.full_calc_on_load());
    let mut expected_changed = vec![sheet_part.clone()];
    if !asks {
        expected_changed.push("xl/workbook.xml".to_owned());
    }
    if had_chain {
        expected_changed.extend([
            "[Content_Types].xml".to_owned(),
            "xl/_rels/workbook.xml.rels".to_owned(),
        ]);
    }
    expected_changed.sort();
    let mut changed = diff.changed.clone();
    changed.sort();
    if changed != expected_changed {
        return Err(format!(
            "changed {changed:?}, expected {expected_changed:?}"
        ));
    }
    if !diff.added.is_empty() {
        return Err(format!("added {:?}", diff.added));
    }
    let removed: &[String] = if had_chain {
        &["xl/calcChain.xml".to_owned()]
    } else {
        &[]
    };
    if diff.removed != removed {
        return Err(format!("removed {:?}", diff.removed));
    }
    let after = package_parts(&saved);
    if !part_text(&saved, "xl/workbook.xml").contains(r#"fullCalcOnLoad="1""#) {
        return Err("the workbook does not ask for a calculation".to_owned());
    }

    // In the sheet, the rest is the bytes it was.
    let (xml_before, xml_after) = (&before[&sheet_part], &after[&sheet_part]);
    let (kept_before, kept_after) = (
        kept_bytes(xml_before, &["cols", "sheetViews"], &[1, 2]),
        kept_bytes(xml_after, &["cols", "sheetViews"], &[1, 2]),
    );
    if kept_before != kept_after {
        return Err("a byte outside the edited children and rows changed".to_owned());
    }

    // What was written.
    let cols = child_of(xml_after, "cols").ok_or("no cols")?;
    let widths: Vec<(u32, u32, f64, bool)> = cols
        .elements_named("col")
        .map(|col| {
            (
                col.tag.u32("min").unwrap(),
                col.tag.u32("max").unwrap(),
                col.tag.f64("width").unwrap(),
                col.tag.bool("customWidth").unwrap_or(false),
            )
        })
        .collect();
    for column in [2, 3] {
        let col = widths
            .iter()
            .find(|(min, max, ..)| (*min..=*max).contains(&column))
            .ok_or_else(|| format!("no col names column {column}"))?;
        if col.2 != 20.7109375 || !col.3 {
            return Err(format!("column {column}: {col:?}"));
        }
    }
    for number in [1, 2] {
        let after = row_of(xml_after, number).ok_or("a row is missing")?;
        if after.ht() != Some(30.0) || !after.custom_height() {
            return Err(format!("row {}: ht {:?}", number + 1, after.ht()));
        }
        if let Some(before) = row_of(xml_before, number)
            && before.children != after.children
        {
            return Err(format!("row {}: its cells changed", number + 1));
        }
    }
    let views = child_of(xml_after, "sheetViews").ok_or("no sheetViews")?;
    for view in views.elements_named("sheetView") {
        let pane = view.child("pane").ok_or("a view without its pane")?;
        let said = |name| pane.tag.get(name);
        let expected = [
            ("xSplit", "1"),
            ("ySplit", "1"),
            ("topLeftCell", "B2"),
            ("activePane", "bottomRight"),
            ("state", "frozen"),
        ];
        if expected
            .iter()
            .any(|(name, value)| said(name).as_deref() != Some(value))
        {
            return Err(format!("the pane is {:?}", pane.tag));
        }
        let selections: Vec<_> = view
            .elements_named("selection")
            .map(|selection| selection.tag.get("pane").unwrap_or_default())
            .collect();
        if selections != ["topRight", "bottomLeft", "bottomRight"] {
            return Err(format!("selections {selections:?}"));
        }
        if view.tag.get("topLeftCell").is_some() {
            return Err("the view keeps its topLeftCell".to_owned());
        }
        if view
            .children
            .iter()
            .position(|node| matches!(node, Node::Element(e) if e.tag.is("pane")))
            != Some(0)
        {
            return Err("the pane is not the first child".to_owned());
        }
    }

    // The same edits again change nothing, and what was saved reads back as
    // itself.
    let mut again = Workbook::from_bytes(&saved).map_err(|e| e.to_string())?;
    apply(&mut again.sheet_at_mut(position).unwrap(), &OPS).map_err(|e| e.to_string())?;
    if again.to_bytes().map_err(|e| e.to_string())? != saved {
        return Err("the edits again changed the workbook".to_owned());
    }
    Ok(had_chain)
}

#[test]
fn every_workbook_takes_widths_heights_and_panes_in_its_first_sheet_and_nothing_else_changes() {
    let files = corpus_workbooks();
    let outcomes = map_in_parallel(&files, |file| edited_sheet(&file.bytes()));
    let mut failures = Vec::new();
    let mut chains = 0;
    for (file, outcome) in files.iter().zip(outcomes) {
        match outcome.unwrap_or_else(|| Err("panicked".to_owned())) {
            Ok(had_chain) => chains += usize::from(had_chain),
            Err(error) => failures.push(format!("{}: {error}", file.path)),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert!(
        chains > 0,
        "no workbook of the corpus has a calculation chain"
    );
}
