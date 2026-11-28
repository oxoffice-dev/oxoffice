//! The resolved style of a cell: font, fill, borders, alignment and style
//! name, colours resolved through the theme, the same in every producer's
//! copy of `budget` (`styles.tsv`) but for what a producer writes otherwise.

use oxoffice_xlsx::{Color, ColorSource, Fill, ResolvedStyle, Workbook};

use crate::support::{corpus_bytes, edited, with_part};

/// A style in one line: `font | fill | left/right/top/bottom | alignment |
/// style name`.
fn summary(style: &ResolvedStyle) -> String {
    let font = &style.font;
    let mut line = format!(
        "{} {}",
        font.name.as_deref().unwrap_or("-"),
        font.size.map_or("-".to_owned(), |size| size.to_string())
    );
    for (set, name) in [(font.bold, "bold"), (font.italic, "italic")] {
        if set {
            line.push_str(&format!(" {name}"));
        }
    }
    let rgb = |color: &Option<Color>| {
        color
            .as_ref()
            .and_then(|color| color.rgb.clone())
            .unwrap_or_else(|| "auto".to_owned())
    };
    line.push_str(&format!(" {} | ", rgb(&font.color)));
    match &style.fill {
        Fill::Pattern {
            pattern,
            foreground,
            ..
        } => line.push_str(&format!("{pattern} {}", rgb(foreground))),
        Fill::Gradient { kind, .. } => line.push_str(kind),
    }
    let border = &style.border;
    let alignment = &style.alignment;
    line.push_str(&format!(
        " | {}/{}/{}/{} | {} {}",
        border.left.style,
        border.right.style,
        border.top.style,
        border.bottom.style,
        alignment.horizontal,
        alignment.vertical
    ));
    if alignment.wrap_text {
        line.push_str(" wrap");
    }
    if alignment.indent > 0 {
        line.push_str(&format!(" indent {}", alignment.indent));
    }
    line.push_str(&format!(
        " | {}",
        style.style_name.as_deref().unwrap_or("-")
    ));
    line
}

/// Every copy of `budget` reads the styles of `styles.tsv`.
#[test]
fn budget_styles_resolve_in_every_copy() {
    let table: Vec<Vec<&str>> = include_str!("styles.tsv")
        .lines()
        .filter(|line| !line.starts_with('#'))
        .map(|line| line.split('\t').collect())
        .collect();
    let mut failures = Vec::new();
    for producer in ["authored", "excel", "google", "libreoffice"] {
        let workbook =
            Workbook::from_bytes(&corpus_bytes(&format!("{producer}/budget.xlsx"))).unwrap();
        for row in table.iter().filter(|row| row[0] == "*") {
            let expected = table
                .iter()
                .find(|other| other[0] == producer && other[1] == row[1])
                .map_or(row[2], |other| other[2]);
            let (sheet, cell) = row[1].split_once('!').unwrap();
            let read = summary(&workbook.sheet(sheet).unwrap().cell(cell).unwrap().style);
            if read != expected {
                failures.push(format!("{producer} {}: {read}", row[1]));
            }
        }
        let color = workbook
            .sheet("Formats")
            .unwrap()
            .cell("C2")
            .unwrap()
            .style
            .font
            .color
            .clone();
        assert_eq!(
            color.map(|color| (color.rgb, color.source)),
            Some((Some("000000".to_owned()), ColorSource::Theme(1))),
            "{producer}: the default font is dark 1, theme index 1"
        );
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn a_hyperlink_takes_the_theme_link_colour_and_its_style_name() {
    for (path, rgb, name) in [
        ("authored/review.xlsx", "0000FF", "Hyperlink"),
        ("excel/review.xlsx", "0000FF", "Lien hypertexte"),
        ("excel/strict.xlsx", "0000FF", "Lien hypertexte"),
        ("google/review.xlsx", "0000FF", "Normal"),
        ("libreoffice/review.xlsx", "0000FF", "*unknown*"),
    ] {
        let workbook = Workbook::from_bytes(&corpus_bytes(path)).unwrap();
        let style = workbook.sheet("Orders").unwrap().cell("A40").unwrap().style;
        let color = style.font.color.clone().unwrap();
        assert_eq!(
            (
                color.rgb.as_deref(),
                color.source,
                style.font.underline.as_deref(),
                style.style_name.as_deref()
            ),
            (
                Some(rgb),
                ColorSource::Theme(10),
                Some("single"),
                Some(name)
            ),
            "{path}"
        );
    }
}

#[test]
fn a_cell_the_sheet_does_not_write_takes_its_row_style_then_its_column_style() {
    let workbook = edited(
        "excel/review.xlsx",
        "xl/worksheets/sheet1.xml",
        &[
            (
                r#"<row r="40" spans="1:7""#,
                r#"<row r="40" spans="1:7" customFormat="1" s="1""#,
            ),
            (
                r#"<col min="5" max="7" width="14.6640625" customWidth="1"/>"#,
                r#"<col min="5" max="7" width="14.6640625" customWidth="1" style="3"/>"#,
            ),
        ],
    );
    let mut workbook = workbook;
    // Reads take the rows and columns an edit staged: a width that splits
    // the styled columns, a height on the styled row.
    let mut orders = workbook.sheet_mut("Orders").unwrap();
    orders.set_column_width("F", 30.0).unwrap();
    orders.set_row_height("40", 30.0).unwrap();
    let sheet = workbook.sheet("Orders").unwrap();
    // (cell, number format, bold, style name): record 1 is bold, 3 is
    // `#,##0.00`, 6 the hyperlink.
    for (cell, format, bold, name) in [
        ("B40", "General", true, "Normal"),
        ("E40", "General", true, "Normal"),
        ("E50", "#,##0.00", false, "Normal"),
        ("F50", "#,##0.00", false, "Normal"),
        ("G50", "#,##0.00", false, "Normal"),
        ("C50", "General", false, "Normal"),
        ("A40", "General", false, "Lien hypertexte"),
    ] {
        let style = sheet.cell(cell).unwrap().style;
        assert_eq!(
            (
                style.number_format.as_str(),
                style.font.bold,
                style.style_name.as_deref()
            ),
            (format, bold, Some(name)),
            "{cell}"
        );
    }
}

#[test]
fn a_theme_that_does_not_read_leaves_its_colours_unresolved() {
    // A colour scheme without its colours, nor the font and format schemes.
    let workbook = with_part(
        "excel/review.xlsx",
        "xl/theme/theme1.xml",
        concat!(
            r#"<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">"#,
            r#"<a:themeElements><a:clrScheme name="Thin"><a:dk1><a:srgbClr val="112233"/></a:dk1>"#,
            r#"</a:clrScheme></a:themeElements></a:theme>"#
        )
        .as_bytes(),
    );
    let cell = workbook.sheet("Orders").unwrap().cell("A1").unwrap();
    assert_eq!(cell.text, "Item");
    let color = cell.style.font.color.clone().unwrap();
    assert_eq!((color.rgb, color.source), (None, ColorSource::Theme(1)));
}
