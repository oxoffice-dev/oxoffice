//! The resolved style of a cell: font, fill, borders, alignment and style
//! name, colours resolved through the theme, the same in every producer's
//! copy of `budget` but for what a producer writes otherwise.

use oxoffice_xlsx::{Color, ColorSource, Fill, ResolvedStyle, Workbook};

use crate::support::corpus_bytes;

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

/// `(sheet!cell, summary)` of `budget`, as the source and every copy read
/// them.
const BUDGET: &[(&str, &str)] = &[
    (
        "Budget!A1",
        "Cambria 16 bold FFFFFF | solid 0F766E | none/none/none/none | center center | Normal",
    ),
    (
        "Budget!A2",
        "Calibri 11 italic 6B7280 | none auto | none/none/none/none | general bottom | Normal",
    ),
    (
        "Budget!A3",
        "Calibri 11 bold FFFFFF | solid 1F2937 | thin/thin/thin/thin | center center wrap | Normal",
    ),
    (
        "Budget!A4",
        "Calibri 11 000000 | none auto | thin/thin/thin/thin | left bottom indent 1 | Normal",
    ),
    (
        "Budget!A12",
        "Calibri 11 bold 000000 | none auto | none/none/thin/double | left bottom indent 1 | Normal",
    ),
    (
        "Budget!A17",
        "Calibri 11 italic 374151 | solid F3F4F6 | medium/medium/medium/medium | general top wrap | Normal",
    ),
    (
        "Formats!C2",
        "Calibri 11 000000 | none auto | none/none/none/none | right bottom | Normal",
    ),
    (
        "Ledger!A1",
        "Calibri 14 bold 000000 | none auto | none/none/none/none | general bottom | Normal",
    ),
];

/// `(copy, sheet!cell, summary, why)`.
const DIFFERENCES: &[(&str, &str, &str, &str)] = &[
    (
        "google",
        "Budget!A4",
        "Calibri 11 000000 | none auto | thin/thin/thin/thin | left bottom | Normal",
        "Google drops the indent",
    ),
    (
        "google",
        "Budget!A12",
        "Calibri 11 bold 000000 | none auto | none/none/thin/double | left bottom | Normal",
        "Google drops the indent",
    ),
    (
        "google",
        "Budget!A17",
        "Calibri 11 italic 374151 | solid F3F4F6 | medium/none/medium/none | general top wrap | Normal",
        "Google gives each cell of a merged range its part of the border",
    ),
];

#[test]
fn budget_styles_resolve_in_every_copy() {
    let mut failures = Vec::new();
    for producer in ["authored", "excel", "google", "libreoffice"] {
        let workbook =
            Workbook::from_bytes(&corpus_bytes(&format!("{producer}/budget.xlsx"))).unwrap();
        for (at, expected) in BUDGET {
            let expected = DIFFERENCES
                .iter()
                .find(|row| row.0 == producer && row.1 == *at)
                .map_or(*expected, |row| row.2);
            let (sheet, cell) = at.split_once('!').unwrap();
            let read = summary(&workbook.sheet(sheet).unwrap().cell(cell).unwrap().style);
            if read != expected {
                failures.push(format!("{producer} {at}: {read}"));
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
