//! The case table: every row is a code and a value written in a workbook,
//! with the text Excel displayed for it (Excel for Mac 16.115, en-US).
//! `ours` is set where this crate shows something else on purpose, and
//! `why` says why. `#####` stands for a cell Excel fills with `#`. Built by
//! `handoff/xlsx/numfmt/pin.py`; see specs/2026-11-27-number-format-codes.md.

use oxoffice_numfmt::{
    Color, DateKind, DateSystem, Fill, FormatOptions, Locale, NumberFormat, Space, builtin_code,
    is_date_format,
};

fn format_of(code: &str) -> NumberFormat {
    let code = match code.strip_prefix("builtin:") {
        Some(id) => builtin_code(id.parse().expect("an id"), Locale::EnUs).unwrap_or("General"),
        None => code,
    };
    // Excel shows a code it cannot read as General.
    NumberFormat::parse(code).unwrap_or_else(|_| NumberFormat::general())
}

#[test]
fn every_case_shows_what_excel_shows() {
    let mut failures = Vec::new();
    let table = include_str!("cases.tsv");
    for line in table.lines().skip(1) {
        let fields: Vec<&str> = line.split('\t').collect();
        let [id, code, value, system, kind, excel, ours, _why] = fields[..] else {
            panic!("a row has eight fields: {line}");
        };
        let format = format_of(code);
        let options = FormatOptions::default().with_date_system(match system {
            "1904" => DateSystem::Date1904,
            _ => DateSystem::Date1900,
        });
        let shown = match kind {
            "number" => format.format_number(value.parse().expect("a number"), &options),
            "text" => format.format_text(value),
            // Booleans are not formatted: the caller writes TRUE or FALSE.
            _ => continue,
        };
        let expected = if ours.is_empty() { excel } else { ours };
        if shown.text != expected || shown.overflow != (expected == "#####") {
            failures.push(format!(
                "{id} {code:?} {value}: {:?}, expected {expected:?}",
                shown.text
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} cases differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn a_render_gets_the_colour_the_fill_and_the_spaces() {
    let options = FormatOptions::default();
    let accounting = format_of("builtin:44").format_number(-1234.5, &options);
    assert_eq!(accounting.text, " $(1,234.50)");
    assert_eq!(
        accounting.fill,
        Some(Fill {
            character: ' ',
            at: 2
        })
    );
    assert_eq!(
        accounting.spaces,
        vec![Space {
            at: 0,
            width_of: '('
        }]
    );
    let red = format_of("0.00;[Red]-0.00").format_number(-5.0, &options);
    assert_eq!(red.color.map(Color::number), Some(3));
    assert_eq!(red.color.map(Color::indexed), Some(10));
    let text = format_of("0;0;0;[Blue]\"<\"@*-").format_text("x");
    assert_eq!(
        (text.text.as_str(), text.color.map(Color::number)),
        ("<x", Some(5))
    );
    assert_eq!(
        text.fill,
        Some(Fill {
            character: '-',
            at: 2
        })
    );
    let negative_date = format_of("m/d/yyyy").format_number(-1.0, &options);
    assert!(negative_date.overflow);
    assert_eq!(negative_date.fill.map(|fill| fill.character), Some('#'));
}

#[test]
fn a_code_says_whether_it_shows_a_date_a_time_or_a_duration() {
    for (code, kind) in [
        ("m/d/yyyy", Some(DateKind::Date)),
        ("h:mm AM/PM", Some(DateKind::Time)),
        ("yyyy-mm-dd hh:mm:ss", Some(DateKind::DateTime)),
        ("[h]:mm:ss", Some(DateKind::Duration)),
        ("[$-F800]dddd, mmmm dd, yyyy", Some(DateKind::Date)),
        ("0.00;[Red]-0.00", None),
        ("\"d\"0", None),
        ("@", None),
    ] {
        assert_eq!(format_of(code).date_kind(), kind, "{code}");
        assert_eq!(is_date_format(code), kind.is_some(), "{code}");
    }
}
