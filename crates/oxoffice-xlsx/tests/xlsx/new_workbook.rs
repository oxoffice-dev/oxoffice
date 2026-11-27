//! A new workbook is the same bytes every time and reads back as it was
//! made; a sheet added to any workbook of the corpus changes the parts a
//! sheet must change and no other.

use oxoffice_core::app_properties::AppProperties;
use oxoffice_opc::OpcPackage;
use oxoffice_xlsx::{DateSystem, Error, SheetKind, SheetState, Workbook, WorkbookClass};
use oxoffice_xlsx_oxml::namespaces::{
    MAIN, STRICT_MAIN, rel_types, transitional_relationship_type,
};
use oxoffice_xlsx_oxml::styles::StylesXml;
use oxoffice_xlsx_oxml::workbook::WorkbookXml;
use std::io::Cursor;
use std::sync::Arc;

use crate::support::{
    corpus_workbooks, holds_formula, map_in_parallel, package_parts, part_text, parts_diff,
};

fn names(workbook: &Workbook) -> Vec<String> {
    workbook
        .sheets()
        .map(|sheet| sheet.name().to_owned())
        .collect()
}

#[test]
fn a_new_workbook_is_the_same_bytes_every_time_and_reads_back_as_it_was_made() {
    let bytes = Workbook::new().to_bytes().unwrap();
    assert_eq!(Workbook::new().to_bytes().unwrap(), bytes);
    let other_thread = std::thread::spawn(|| Workbook::new().to_bytes().unwrap());
    assert_eq!(other_thread.join().unwrap(), bytes);
    assert_eq!(
        Workbook::with_sheet_names(&["Sheet1"])
            .unwrap()
            .to_bytes()
            .unwrap(),
        bytes
    );

    let parts = package_parts(&bytes);
    assert_eq!(
        parts.keys().map(String::as_str).collect::<Vec<_>>(),
        [
            "[Content_Types].xml",
            "_rels/.rels",
            "docProps/app.xml",
            "docProps/core.xml",
            "xl/_rels/workbook.xml.rels",
            "xl/styles.xml",
            "xl/theme/theme1.xml",
            "xl/workbook.xml",
            "xl/worksheets/sheet1.xml",
        ]
    );

    let reread = Workbook::from_bytes(&bytes).unwrap();
    assert_eq!(names(&reread), ["Sheet1"]);
    assert_eq!(reread.class(), WorkbookClass::Workbook);
    assert_eq!(reread.date_system(), DateSystem::Date1900);
    let sheet = reread.sheet_at(0).unwrap();
    assert_eq!(
        (sheet.kind(), sheet.state()),
        (SheetKind::Worksheet, SheetState::Visible)
    );
    assert_eq!(reread.to_bytes().unwrap(), bytes);

    // Calibri 11 and the records Excel always writes: one font, the two
    // fills, one border, one cell format, the Normal style.
    let styles = StylesXml::parse(Arc::new(parts["xl/styles.xml"].clone())).unwrap();
    let counts: Vec<_> = styles
        .part()
        .lists()
        .iter()
        .map(|list| (list.name().to_owned(), list.len()))
        .collect();
    let counts: Vec<_> = counts
        .iter()
        .map(|(name, count)| (name.as_str(), *count))
        .collect();
    assert_eq!(
        counts,
        [
            ("numFmts", 0),
            ("fonts", 1),
            ("fills", 2),
            ("borders", 1),
            ("cellStyleXfs", 1),
            ("cellXfs", 1),
            ("cellStyles", 1)
        ]
    );
    let font = styles.part().lists()[1].get(0).unwrap();
    let said = |name: &str| font.child(name).and_then(|element| element.tag.get("val"));
    assert_eq!(
        (
            said("name").as_deref(),
            said("sz").as_deref(),
            said("scheme").as_deref()
        ),
        (Some("Calibri"), Some("11"), Some("minor"))
    );
    assert_eq!(styles.cell_styles().next().unwrap().name(), "Normal");

    // The Office 2013-2022 theme, its fonts and its accent.
    let theme = part_text(&bytes, "xl/theme/theme1.xml");
    for expected in [
        r#"name="Office Theme""#,
        r#"<a:latin typeface="Calibri Light""#,
        r#"<a:latin typeface="Calibri" panose"#,
        r#"<a:accent1><a:srgbClr val="4472C4"/>"#,
    ] {
        assert!(theme.contains(expected), "{expected}");
    }

    // oxoffice, in the form of version the schema asks of AppVersion.
    let app = AppProperties::from_xml(&parts["docProps/app.xml"]).unwrap();
    assert_eq!(app.application.as_deref(), Some("oxoffice"));
    let version = app.application_version.unwrap();
    let (major, minor) = version.split_once('.').unwrap();
    assert!(
        major.len() == 2
            && minor.len() == 4
            && (major.to_owned() + minor)
                .bytes()
                .all(|b| b.is_ascii_digit()),
        "{version}"
    );
}

#[test]
fn a_new_workbook_has_the_sheets_it_is_given_and_refuses_the_names_excel_would() {
    let workbook = Workbook::with_sheet_names(&["Data", "q & a <1>", "Summary"]).unwrap();
    let bytes = workbook.to_bytes().unwrap();
    let reread = Workbook::from_bytes(&bytes).unwrap();
    assert_eq!(names(&reread), ["Data", "q & a <1>", "Summary"]);
    let parts = package_parts(&bytes);
    let sheets = WorkbookXml::parse(Arc::new(parts["xl/workbook.xml"].clone())).unwrap();
    let ids: Vec<_> = sheets
        .sheets()
        .map(|sheet| sheet.sheet_id().unwrap())
        .collect();
    assert_eq!(ids, [1, 2, 3]);
    // Only the first sheet is in view.
    for (number, in_view) in [(1, true), (2, false), (3, false)] {
        let sheet = part_text(&bytes, &format!("xl/worksheets/sheet{number}.xml"));
        assert_eq!(sheet.contains("tabSelected"), in_view, "sheet {number}");
    }

    let none: [&str; 0] = [];
    for (names, message) in [
        (vec!["A", "a"], "exists already"),
        (vec!["A", "b:c"], "holds ':'"),
        (vec![""], "empty"),
    ] {
        let error = Workbook::with_sheet_names(&names).unwrap_err();
        assert!(error.to_string().contains(message), "{names:?}: {error}");
    }
    assert!(
        Workbook::with_sheet_names(&none)
            .unwrap_err()
            .to_string()
            .contains("at least one sheet")
    );
}

#[test]
fn a_sheet_added_takes_the_next_ids_and_a_refused_name_changes_nothing() {
    let mut workbook = Workbook::new();
    let before = workbook.to_bytes().unwrap();
    for name in ["sheet1", "History", "a/b", &"x".repeat(32)] {
        let error = workbook.add_sheet(name).unwrap_err();
        assert!(
            matches!(error, Error::InvalidArgument(_) | Error::Refused(_)),
            "{name}"
        );
    }
    assert_eq!(workbook.to_bytes().unwrap(), before);

    assert_eq!(workbook.add_sheet("Data").unwrap().position(), 1);
    assert_eq!(workbook.add_sheet("Q_x0041_").unwrap().position(), 2);
    let saved = workbook.to_bytes().unwrap();
    let reread = Workbook::from_bytes(&saved).unwrap();
    // A literal `_x0041_` is written escaped, and read back as it was.
    assert_eq!(names(&reread), ["Sheet1", "Data", "Q_x0041_"]);
    assert!(part_text(&saved, "xl/workbook.xml").contains(r#"name="Q_x005F_x0041_""#));
    let diff = parts_diff(&before, &saved);
    assert_eq!(
        diff.added,
        ["xl/worksheets/sheet2.xml", "xl/worksheets/sheet3.xml"]
    );
    // No formula, so nothing to calculate: Excel is not asked to.
    assert!(part_text(&saved, "xl/workbook.xml").contains(r#"<calcPr calcId="191029"/>"#));
}

#[test]
fn a_sheet_name_with_markup_and_other_characters_is_escaped_once_and_reads_back_as_given() {
    let given = [
        "R&D",
        "a<b>\"c\"",
        "it's",
        "\u{c9}t\u{e9}",
        "x&amp;y",
        "Q_x0041_",
    ];
    // Added one by one, and given at once.
    let mut added = Workbook::new();
    for name in given {
        added.add_sheet(name).unwrap();
    }
    let all: Vec<&str> = ["Sheet1"].into_iter().chain(given).collect();
    let at_once = Workbook::with_sheet_names(&all).unwrap();
    for (how, workbook) in [("added", &added), ("at once", &at_once)] {
        let saved = workbook.to_bytes().unwrap();
        assert_eq!(names(&Workbook::from_bytes(&saved).unwrap()), all, "{how}");
        let text = part_text(&saved, "xl/workbook.xml");
        for written in [
            r#"name="R&amp;D""#,
            r#"name="a&lt;b&gt;&quot;c&quot;""#,
            r#"name="it&apos;s""#,
            r#"name="x&amp;amp;y""#,
            r#"name="Q_x005F_x0041_""#,
        ] {
            assert!(text.contains(written), "{how}: {written}");
        }
    }
}

/// Adds a sheet to `original` and checks what changed. Gives whether the
/// workbook holds a formula, and whether it had a calculation chain.
fn with_sheet_added(original: &[u8]) -> Result<(bool, bool), String> {
    let mut workbook = Workbook::from_bytes(original).map_err(|e| e.to_string())?;
    let before = names(&workbook);
    workbook.add_sheet("Added").map_err(|e| e.to_string())?;
    let saved = workbook.to_bytes().map_err(|e| e.to_string())?;
    let (parts, after) = (package_parts(original), package_parts(&saved));
    let diff = parts_diff(original, &saved);

    let had_chain = parts.contains_key("xl/calcChain.xml");
    let formula = holds_formula(&parts);
    let had_app_lists = parts.get("docProps/app.xml").is_some_and(|app| {
        let app = String::from_utf8_lossy(app);
        app.contains("TitlesOfParts") || app.contains("HeadingPairs")
    });
    let mut expected: Vec<&str> = vec![
        "[Content_Types].xml",
        "xl/_rels/workbook.xml.rels",
        "xl/workbook.xml",
    ];
    if had_app_lists {
        expected.push("docProps/app.xml");
    }
    expected.sort_unstable();
    let mut changed = diff.changed.clone();
    changed.sort();
    if changed != expected {
        return Err(format!("changed {changed:?}, expected {expected:?}"));
    }
    // A workbook that holds a formula is calculated on open and loses its
    // chain; one that holds none is left as it is.
    let removed: &[&str] = if formula && had_chain {
        &["xl/calcChain.xml"]
    } else {
        &[]
    };
    if diff.removed != removed {
        return Err(format!("removed {:?}", diff.removed));
    }
    let [new_part] = diff.added.as_slice() else {
        return Err(format!("added {:?}", diff.added));
    };

    // The sheet is a worksheet in the namespace of its package.
    let reread = Workbook::from_bytes(&saved).map_err(|e| e.to_string())?;
    let mut expected_names = before.clone();
    expected_names.push("Added".to_owned());
    if names(&reread) != expected_names {
        return Err(format!("sheets {:?}", names(&reread)));
    }
    let added = reread.sheet_at(before.len()).unwrap();
    if (added.kind(), added.state()) != (SheetKind::Worksheet, SheetState::Visible) {
        return Err("the sheet is not a visible worksheet".to_owned());
    }
    let workbook_xml = WorkbookXml::parse(Arc::new(after["xl/workbook.xml"].clone()))
        .map_err(|e| e.to_string())?;
    let namespace = if workbook_xml.is_strict() {
        STRICT_MAIN
    } else {
        MAIN
    };
    let sheet = String::from_utf8_lossy(&after[new_part]);
    if !sheet.contains(&format!(r#"<worksheet xmlns="{namespace}""#)) {
        return Err(format!("the new part is not in {namespace}"));
    }

    // Its record, its relationship and its content type.
    let ids: Vec<u32> = workbook_xml
        .sheets()
        .filter_map(|sheet| sheet.sheet_id())
        .collect();
    let (new_id, old_ids) = ids.split_last().unwrap();
    if *new_id != old_ids.iter().max().copied().unwrap_or(0) + 1 {
        return Err(format!("sheetIds {ids:?}"));
    }
    let package = OpcPackage::from_reader(Cursor::new(&saved)).map_err(|e| e.to_string())?;
    let record = workbook_xml.sheets().last().unwrap();
    let relationship = package
        .get_part_rels("/xl/workbook.xml")
        .and_then(|relationships| relationships.get_by_id(&record.relationship_id().unwrap()))
        .ok_or("the relationship of the sheet is missing")?;
    if transitional_relationship_type(&relationship.rel_type) != rel_types::WORKSHEET
        || OpcPackage::resolve_rel_target("/xl/workbook.xml", &relationship.target)
            != format!("/{new_part}")
    {
        return Err(format!("the relationship is {relationship:?}"));
    }
    if package
        .content_types
        .content_type_for(&format!("/{new_part}"))
        != Some(oxoffice_xlsx_oxml::namespaces::content_types::WORKSHEET)
    {
        return Err("the content type of the sheet is not a worksheet's".to_owned());
    }
    if formula && had_chain {
        let chain = |relationship: &oxoffice_opc::Relationship| {
            transitional_relationship_type(&relationship.rel_type) == rel_types::CALC_CHAIN
        };
        let kept = package
            .get_part_rels("/xl/workbook.xml")
            .is_some_and(|relationships| relationships.items.iter().any(chain));
        if kept || package.content_types.contains_override("/xl/calcChain.xml") {
            return Err("the calculation chain is not removed whole".to_owned());
        }
    }
    let asked = |bytes: &[u8]| {
        String::from_utf8_lossy(bytes).contains("fullCalcOnLoad=\"1\"")
            || String::from_utf8_lossy(bytes).contains("fullCalcOnLoad=\"true\"")
    };
    if asked(&after["xl/workbook.xml"]) != (asked(&parts["xl/workbook.xml"]) || formula) {
        return Err(format!("calculation on open: asked {formula} of a formula"));
    }
    if had_app_lists {
        let app = part_text(&saved, "docProps/app.xml");
        if app.contains("TitlesOfParts") || app.contains("HeadingPairs") {
            return Err("app.xml still lists the sheets".to_owned());
        }
    }
    Ok((formula, had_chain))
}

#[test]
fn a_sheet_added_to_every_workbook_changes_the_parts_a_sheet_must_and_no_other() {
    let files = corpus_workbooks();
    let outcomes = map_in_parallel(&files, |file| with_sheet_added(&file.bytes()));
    let mut failures = Vec::new();
    let (mut formulas, mut chains) = (0, 0);
    for (file, outcome) in files.iter().zip(outcomes) {
        match outcome.unwrap_or_else(|| Err("panicked".to_owned())) {
            Ok((formula, had_chain)) => {
                formulas += usize::from(formula);
                chains += usize::from(had_chain);
            }
            Err(error) => failures.push(format!("{}: {error}", file.path)),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert!(
        chains > 0,
        "no workbook of the corpus has a calculation chain"
    );
    assert!(formulas > 0, "no workbook of the corpus holds a formula");
}
