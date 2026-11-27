//! The resolved style of a cell: font, fill, borders, alignment and style
//! name, colours resolved through the theme, the same in every producer's
//! copy of `budget` (`styles.tsv`) but for what a producer writes otherwise.

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
