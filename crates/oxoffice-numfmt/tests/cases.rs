//! The case table: every row is a code and a value written in a workbook,
//! with the text Excel displayed for it (Excel for Mac 16.115, en-US).
//! `ours` is set where this crate shows something else on purpose, and
//! `why` says why. `#####` stands for a cell Excel fills with `#`. Built by
//! the scripts of `tests/excel/` (its README says how a round is run).

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
            kind => panic!("{id}: unknown kind {kind}"),
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
    let shown = |code: &str, value: f64| format_of(code).format_number(value, &options);
    let fill = |character, at| Some(Fill { character, at });
    let space = |at, width_of| Space { at, width_of };

    let accounting = shown("builtin:44", -1234.5);
    assert_eq!(accounting.text, " $(1,234.50)");
    assert_eq!(accounting.fill, fill(' ', 2));
    assert_eq!(accounting.spaces, vec![space(0, '(')]);
    // After the minus sign of a single section, and after a two-byte `€`
    // and a three-byte one: offsets are bytes.
    let after_minus = shown("*-0_)", -5.0);
    assert_eq!(
        (after_minus.text.as_str(), after_minus.fill),
        ("-5 ", fill('-', 1))
    );
    assert_eq!(after_minus.spaces, vec![space(2, ')')]);
    let after_euro = shown("[$€-40C]* 0_€", 5.0);
    assert_eq!(
        (after_euro.text.as_str(), after_euro.fill),
        ("€5 ", fill(' ', 3))
    );
    assert_eq!(after_euro.spaces, vec![space(4, '€')]);
    let after_pound = shown("\"£\"* 0", -5.0);
    assert_eq!(
        (after_pound.text.as_str(), after_pound.fill),
        ("-£5", fill(' ', 3))
    );

    // The colour is the section's, also when a condition picks it.
    let colour = |code: &str, value| shown(code, value).color.map(Color::number);
    assert_eq!(colour("0.00;[Red]-0.00", -5.0), Some(3));
    assert_eq!(
        colour("[Blue][>=100]0;[Red][<0]0;[Color10]0", 150.0),
        Some(5)
    );
    assert_eq!(
        colour("[Blue][>=100]0;[Red][<0]0;[Color10]0", -5.0),
        Some(3)
    );
    assert_eq!(
        colour("[Blue][>=100]0;[Red][<0]0;[Color10]0", 50.0),
        Some(10)
    );
    assert_eq!(colour("0.00;[Red]-0.00", -5.0).map(|n| n + 7), Some(10));
    assert_eq!(
        format_of("0;[Red]-0")
            .format_number(-5.0, &options)
            .color
            .map(Color::indexed),
        Some(10)
    );

    let text = format_of("0;0;0;[Blue]\"<\"@*-").format_text("x");
    assert_eq!(
        (text.text.as_str(), text.color.map(Color::number)),
        ("<x", Some(5))
    );
    assert_eq!(text.fill, fill('-', 2));

    let negative_date = shown("m/d/yyyy", -1.0);
    assert!(negative_date.overflow);
    assert_eq!(negative_date.fill.map(|fill| fill.character), Some('#'));
}

#[test]
fn a_code_as_wide_as_excel_takes_pads_and_a_longer_one_is_general() {
    let options = FormatOptions::default();
    // 253 `h`: the widest `[h]` a code of 255 characters holds.
    let widest = format!("[{}]", "h".repeat(253));
    let shown = format_of(&widest).format_number(1.5, &options);
    assert_eq!(shown.text, format!("{}36", "0".repeat(251)));
    // Far beyond any width `format!` takes: General, as Excel repairs it.
    let absurd = format!("[{}]", "h".repeat(70_000));
    assert!(NumberFormat::parse(&absurd).is_err());
    assert_eq!(format_of(&absurd).format_number(1.5, &options).text, "1.5");
}

#[test]
fn names_in_a_language_without_a_table_are_reported() {
    for (code, unknown) in [
        ("[$-409]mmm-yy", false),
        ("[$-809]dddd", false),
        ("[$-en-US]mmmm", false),
        ("[$-1009]yyyy/mm/dd", false),
        ("[$-407]dddd", false),
        ("[$-fr-FR]mmm", false),
        ("[$-40C]mmmm yyyy", false),
        ("[$-411]mmmm", true),
        ("[$-ja-JP]ddd", true),
        ("[$-411]h:mm AM/PM", true),
        // Numbers in the tag's language need no names.
        ("[$-411]dd/mm/yyyy", false),
        ("#,##0.00 [$€-411]", false),
    ] {
        assert_eq!(format_of(code).shows_unknown_names(), unknown, "{code}");
    }
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
