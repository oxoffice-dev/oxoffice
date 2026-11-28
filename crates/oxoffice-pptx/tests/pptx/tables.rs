//! Tables on slides: adding one, cell text and formatting, rows and columns,
//! merged cells, the style and its flags, and the 4x3 "Key figures" table of
//! the corpus decks as PowerPoint, Google Slides and LibreOffice saved it.

use oxoffice_pptx::{
    CT_LineProperties, CT_TextCharacterProperties, CT_TextParagraphProperties, CellBorder, Emu,
    Error, Fill, Presentation, ShapeOwner, TableMut, TableRef, TextAlignment, TextAnchor,
};

use crate::support::{corpus_deck, new_deck, reopen};

const INCH: i64 = 914_400;

/// Every producer of the corpus decks.
const PRODUCERS: [&str; 4] = ["powerpoint", "google", "libreoffice", "authored"];

/// Slide and shape index of the "Key figures" table in every corpus deck.
const KEY_FIGURES: (usize, usize) = (2, 1);

/// The "Key figures" table, as every producer shows it.
const KEY_FIGURES_TEXT: [[&str; 3]; 4] = [
    ["Indicator", "Q2", "Q3"],
    ["Revenue", "4,817", "5,362"],
    ["Costs", "3,950", "4,240"],
    ["Margin", "867", "1,122"],
];

/// PowerPoint's built-in "Medium Style 2 - Accent 1", its default for a new
/// table.
const MEDIUM_STYLE_2_ACCENT_1: &str = "{5C22544A-7EE6-4342-B048-85BDC9FD1C3A}";

/// The table style Google Slides defines in its `ppt/tableStyles.xml`.
const GOOGLE_TABLE_STYLE: &str = "{977F85BB-2660-466B-8569-FC4677D6DBE6}";

fn layout(prs: &Presentation, name: &str) -> usize {
    (0..prs.layout_count())
        .find(|&index| prs.layout_name(index) == Some(name))
        .unwrap_or_else(|| panic!("the deck has a {name:?} layout"))
}

/// A new deck with one blank slide holding one table of `rows` by `columns`
/// cells of one inch by half an inch, one inch from the top-left corner.
fn deck_with_table(rows: usize, columns: usize) -> Presentation {
    let mut prs = new_deck();
    prs.add_slide(layout(&prs, "Blank")).unwrap();
    let (width, height) = (columns as i64 * INCH, rows as i64 * INCH / 2);
    prs.slide_mut(0)
        .unwrap()
        .add_table(rows, columns, Emu(INCH), Emu(INCH), Emu(width), Emu(height))
        .unwrap();
    prs
}

/// The table of one shape, for editing.
fn table_mut(prs: &mut Presentation, slide: usize, shape: usize) -> TableMut<'_> {
    prs.slide_mut(slide)
        .expect("the slide exists")
        .into_shape_mut(shape)
        .expect("the shape exists")
        .into_table_mut()
        .expect("the shape is a table")
}

/// The table of one shape.
fn table(prs: &Presentation, slide: usize, shape: usize) -> TableRef<'_> {
    prs.slide(slide)
        .expect("the slide exists")
        .shape(shape)
        .expect("the shape exists")
        .table()
        .expect("the shape is a table")
}

fn frame_size(prs: &Presentation, slide: usize, shape: usize) -> (Emu, Emu) {
    prs.slide(slide)
        .unwrap()
        .shape(shape)
        .unwrap()
        .size()
        .expect("the frame has a size")
}

/// Every cell text, row by row.
fn cell_texts(table: &TableRef<'_>) -> Vec<Vec<String>> {
    (0..table.row_count())
        .map(|row| {
            (0..table.column_count())
                .map(|column| table.cell(row, column).unwrap().text())
                .collect()
        })
        .collect()
}

fn column_widths(table: &TableRef<'_>) -> Vec<Emu> {
    (0..table.column_count())
        .map(|column| table.column_width(column).unwrap())
        .collect()
}

fn row_heights(table: &TableRef<'_>) -> Vec<Emu> {
    (0..table.row_count())
        .map(|row| table.row_height(row).unwrap())
        .collect()
}

/// Writes its coordinates into every cell: `r0c0`, `r0c1`, ...
fn number_cells(table: &mut TableMut<'_>) {
    for row in 0..table.row_count() {
        for column in 0..table.column_count() {
            table
                .cell_mut(row, column)
                .unwrap()
                .set_text(&format!("r{row}c{column}"));
        }
    }
}

/// Whether the cell is neither a merge origin nor covered by a merge.
fn is_plain(table: &TableRef<'_>, row: usize, column: usize) -> bool {
    let cell = table.cell(row, column).unwrap();
    !cell.is_merge_origin()
        && !cell.is_spanned()
        && (cell.span_height(), cell.span_width()) == (1, 1)
}

fn solid(rgb: &str) -> Fill {
    Fill::from_xml(format!(r#"<a:solidFill><a:srgbClr val="{rgb}"/></a:solidFill>"#).as_bytes())
        .unwrap()
}

fn line(rgb: &str) -> CT_LineProperties {
    CT_LineProperties::from_xml(
        format!(r#"<a:ln w="12700"><a:solidFill><a:srgbClr val="{rgb}"/></a:solidFill></a:ln>"#)
            .as_bytes(),
    )
    .unwrap()
}

/// The size, typeface and colour of the first run of a cell.
fn first_run_look(
    table: &TableRef<'_>,
    row: usize,
    column: usize,
) -> (Option<i32>, Option<String>, Option<String>) {
    let cell = table.cell(row, column).unwrap();
    let frame = cell.text_frame().expect("the cell has a text body");
    let paragraph = frame.paragraph(0).expect("the cell has a paragraph");
    let run = paragraph.run(0).expect("the cell has a run");
    (
        run.font_size(),
        run.font_name().map(str::to_owned),
        run.font_color(),
    )
}

fn assert_refused(result: oxoffice_pptx::Result<()>, operation: &str) {
    match result {
        Err(Error::InvalidTableMutation { operation: got, .. }) if got == operation => {}
        other => panic!("{operation}: expected a refusal, got {other:?}"),
    }
}

// Adding a table.

#[test]
fn a_table_needs_rows_columns_and_room_for_every_cell() {
    let mut prs = new_deck();
    prs.add_slide(layout(&prs, "Blank")).unwrap();
    let before = prs.to_bytes().unwrap();
    for (rows, columns, width, height) in [
        (0, 3, 3 * INCH, INCH),
        (2, 0, 3 * INCH, INCH),
        (2, 3, -3 * INCH, INCH),
        (2, 3, 3 * INCH, 0),
        // Two EMU cannot hold three columns.
        (2, 3, 2, INCH),
    ] {
        let result = prs
            .slide_mut(0)
            .unwrap()
            .add_table(rows, columns, Emu(0), Emu(0), Emu(width), Emu(height))
            .map(|_| ());
        assert_refused(result, "add table");
    }
    assert!(
        prs.to_bytes().unwrap() == before,
        "a refused table left a trace"
    );
}

// Cell text.

#[test]
fn unicode_text_in_cells_survives_a_save() {
    const SAMPLES: [&str; 9] = [
        "日本語の表",
        "表格中的文字",
        "مرحبا بالعالم",
        "שלום עולם",
        "👍🏽 👨\u{200d}👩\u{200d}👧",
        "e\u{301}te\u{301}",
        "<b> & \"x\" 'y'",
        "  padded  ",
        "a\tb",
    ];
    let mut prs = deck_with_table(1, SAMPLES.len());
    {
        let mut table = table_mut(&mut prs, 0, 0);
        for (column, text) in SAMPLES.iter().enumerate() {
            table.cell_mut(0, column).unwrap().set_text(text);
        }
    }

    let prs = reopen(&prs);
    assert_eq!(cell_texts(&table(&prs, 0, 0)), [SAMPLES]);
}

// Cell formatting.

#[test]
fn cell_runs_paragraphs_and_anchor_keep_their_formatting_after_a_save() {
    let mut prs = deck_with_table(1, 2);
    {
        let mut table = table_mut(&mut prs, 0, 0);
        let mut cell = table.cell_mut(0, 0).unwrap();
        cell.set_text("Total");
        cell.set_vertical_anchor(Some(TextAnchor::Center));
        let mut frame = cell.text_frame();
        let mut paragraph = frame.paragraph_mut(0).unwrap();
        let mut centred = CT_TextParagraphProperties::default();
        centred.alignment = Some(TextAlignment::Center);
        paragraph.set_properties(centred);
        let mut look = CT_TextCharacterProperties::default();
        look.bold = Some(true);
        look.font_size = Some(2400);
        look.fill = Some(solid("C00000"));
        paragraph.run_mut(0).unwrap().set_properties(look);
        frame.add_paragraph().set_text("in k€");
    }

    let mut prs = reopen(&prs);
    {
        let table = table(&prs, 0, 0);
        let cell = table.cell(0, 0).unwrap();
        assert_eq!(cell.text(), "Total\nin k€");
        assert_eq!(cell.vertical_anchor(), Some(TextAnchor::Center));
        let frame = cell.text_frame().unwrap();
        let paragraph = frame.paragraph(0).unwrap();
        assert_eq!(
            paragraph.properties().and_then(|p| p.alignment),
            Some(TextAlignment::Center)
        );
        let run = paragraph.run(0).unwrap();
        assert_eq!(run.properties().and_then(|p| p.bold), Some(true));
        assert_eq!(run.font_size(), Some(2400));
        assert_eq!(run.font_color().as_deref(), Some("C00000"));
        assert_eq!(table.cell(0, 1).unwrap().vertical_anchor(), None);
    }

    table_mut(&mut prs, 0, 0)
        .cell_mut(0, 0)
        .unwrap()
        .set_vertical_anchor(None);
    let prs = reopen(&prs);
    assert_eq!(
        table(&prs, 0, 0).cell(0, 0).unwrap().vertical_anchor(),
        None
    );
}

#[test]
fn a_cell_fill_is_set_replaced_made_transparent_and_cleared() {
    let mut prs = deck_with_table(2, 2);
    {
        let mut table = table_mut(&mut prs, 0, 0);
        table
            .cell_mut(0, 0)
            .unwrap()
            .set_fill(Some(solid("1F4E79")));
        table
            .cell_mut(0, 1)
            .unwrap()
            .set_fill(Some(solid("1F4E79")));
    }
    let mut prs = reopen(&prs);
    {
        let table = table(&prs, 0, 0);
        assert_eq!(table.cell(0, 0).unwrap().fill(), Some(&solid("1F4E79")));
        assert_eq!(table.cell(1, 0).unwrap().fill(), None);
    }

    {
        let mut table = table_mut(&mut prs, 0, 0);
        table
            .cell_mut(0, 0)
            .unwrap()
            .set_fill(Some(solid("FFC000")));
        table.cell_mut(0, 1).unwrap().set_fill(None);
        // No fill is a fill of its own: the cell shows what lies behind it
        // instead of the table style.
        table
            .cell_mut(1, 1)
            .unwrap()
            .set_fill(Some(Fill::from_xml(b"<a:noFill/>").unwrap()));
    }
    let prs = reopen(&prs);
    let table = table(&prs, 0, 0);
    assert_eq!(table.cell(0, 0).unwrap().fill(), Some(&solid("FFC000")));
    assert_eq!(table.cell(0, 1).unwrap().fill(), None);
    assert!(matches!(
        table.cell(1, 1).unwrap().fill(),
        Some(Fill::NoFill(_))
    ));
}

#[test]
fn cell_margins_and_borders_are_set_one_side_at_a_time() {
    let mut prs = deck_with_table(2, 2);
    {
        let mut table = table_mut(&mut prs, 0, 0);
        let mut cell = table.cell_mut(0, 0).unwrap();
        cell.set_margins(Some(Emu(0)), Some(Emu(45_720)), None, Some(Emu(91_440)));
        cell.set_border(CellBorder::Left, Some(line("FF0000")));
        cell.set_border(CellBorder::Bottom, Some(line("00B050")));
        cell.set_border(CellBorder::Top, Some(line("0000FF")));
        cell.set_border(CellBorder::Top, None);
    }

    let prs = reopen(&prs);
    let table = table(&prs, 0, 0);
    let cell = table.cell(0, 0).unwrap();
    assert_eq!(
        cell.margins(),
        (Some(Emu(0)), Some(Emu(45_720)), None, Some(Emu(91_440)))
    );
    let edges = [
        CellBorder::Left,
        CellBorder::Right,
        CellBorder::Top,
        CellBorder::Bottom,
    ];
    assert_eq!(
        edges.map(|edge| cell.border(edge).cloned()),
        [Some(line("FF0000")), None, None, Some(line("00B050"))]
    );
    let other = table.cell(1, 1).unwrap();
    assert_eq!(other.margins(), (None, None, None, None));
    assert!(edges.iter().all(|&edge| other.border(edge).is_none()));
}

#[test]
fn a_hyperlink_on_cell_text_survives_a_save() {
    let mut prs = deck_with_table(1, 2);
    table_mut(&mut prs, 0, 0)
        .cell_mut(0, 1)
        .unwrap()
        .set_text("Docs");
    let id = prs
        .slide(0)
        .unwrap()
        .shape(0)
        .unwrap()
        .non_visual_id()
        .unwrap();
    prs.set_cell_run_hyperlink(0, id, (0, 1), 0, 0, Some("https://example.com/docs"))
        .unwrap();
    // The empty cell has no run to carry a link.
    assert!(
        prs.set_cell_run_hyperlink(0, id, (0, 0), 0, 0, Some("https://example.com/"))
            .is_err()
    );

    let prs = reopen(&prs);
    let table = table(&prs, 0, 0);
    let cell = table.cell(0, 1).unwrap();
    let frame = cell.text_frame().unwrap();
    let paragraph = frame.paragraph(0).unwrap();
    let run = paragraph.run(0).unwrap();
    assert_eq!(run.text(), "Docs");
    let link = run
        .properties()
        .and_then(|p| p.hyperlink_click.as_ref())
        .and_then(|link| link.relationship_id.as_deref())
        .expect("the run links somewhere");
    assert_eq!(
        prs.hyperlink_address(0, link),
        Some("https://example.com/docs")
    );
}

// Column widths and row heights.

#[test]
fn invalid_sizes_and_indexes_are_refused_and_change_nothing() {
    let mut prs = deck_with_table(2, 3);
    let before = prs.to_bytes().unwrap();
    {
        let mut table = table_mut(&mut prs, 0, 0);
        assert_refused(table.set_column_width(0, Emu(0)), "set column width");
        assert_refused(table.set_column_width(0, Emu(-INCH)), "set column width");
        assert_refused(table.set_column_width(3, Emu(INCH)), "set column width");
        // A width past the EMU range overflows the frame.
        assert_refused(table.set_column_width(0, Emu(i64::MAX)), "set column width");
        assert_refused(table.set_row_height(0, Emu(0)), "set row height");
        assert_refused(table.set_row_height(2, Emu(INCH)), "set row height");
        assert_refused(table.insert_row(3), "insert row");
        assert_refused(table.remove_row(2), "remove row");
        assert_refused(table.insert_column(4), "insert column");
        assert_refused(table.remove_column(3), "remove column");
        assert!(table.cell_mut(2, 0).is_none() && table.cell_mut(0, 3).is_none());
    }
    assert!(
        prs.to_bytes().unwrap() == before,
        "a refused edit changed the deck"
    );
}

// Rows and columns.

#[test]
fn an_inserted_row_or_column_copies_its_neighbour_formatting_without_its_text() {
    let mut prs = deck_with_table(2, 2);
    {
        let mut table = table_mut(&mut prs, 0, 0);
        for (column, heading) in ["Name", "Score"].into_iter().enumerate() {
            let mut cell = table.cell_mut(0, column).unwrap();
            cell.set_text(heading);
            cell.set_fill(Some(solid("1F4E79")));
            let mut look = CT_TextCharacterProperties::default();
            look.bold = Some(true);
            look.fill = Some(solid("FFFFFF"));
            cell.text_frame()
                .paragraph_mut(0)
                .unwrap()
                .run_mut(0)
                .unwrap()
                .set_properties(look);
        }
        table.cell_mut(1, 0).unwrap().set_text("Ada");
        table.cell_mut(1, 1).unwrap().set_text("12");
        table.set_row_height(0, Emu(INCH)).unwrap();
        table.set_column_width(0, Emu(2 * INCH)).unwrap();

        table.insert_row(1).unwrap();
        table.insert_column(1).unwrap();
        // Typed into the new header cell, text takes the copied formatting.
        table.cell_mut(1, 0).unwrap().set_text("Typed");
    }

    let prs = reopen(&prs);
    let table = table(&prs, 0, 0);
    assert_eq!(
        cell_texts(&table),
        [["Name", "", "Score"], ["Typed", "", ""], ["Ada", "", "12"]]
    );
    assert_eq!(row_heights(&table), [INCH, INCH, INCH / 2].map(Emu));
    assert_eq!(column_widths(&table), [2 * INCH, 2 * INCH, INCH].map(Emu));
    assert_eq!(frame_size(&prs, 0, 0), (Emu(5 * INCH), Emu(5 * INCH / 2)));
    for (row, column) in [(1, 0), (1, 1), (1, 2), (0, 1)] {
        assert_eq!(
            table.cell(row, column).unwrap().fill(),
            Some(&solid("1F4E79")),
            "cell {row},{column} copies the header fill"
        );
    }
    assert_eq!(table.cell(2, 1).unwrap().fill(), None);
    let typed = table.cell(1, 0).unwrap();
    let frame = typed.text_frame().unwrap();
    let paragraph = frame.paragraph(0).unwrap();
    let run = paragraph.run(0).unwrap();
    assert_eq!(run.properties().and_then(|p| p.bold), Some(true));
    assert_eq!(run.font_color().as_deref(), Some("FFFFFF"));
}

#[test]
fn the_last_row_and_the_last_column_cannot_be_removed() {
    let mut prs = deck_with_table(1, 2);
    {
        let mut table = table_mut(&mut prs, 0, 0);
        table.cell_mut(0, 1).unwrap().set_text("kept");
        assert_refused(table.remove_row(0), "remove row");
        table.remove_column(0).unwrap();
        assert_refused(table.remove_column(0), "remove column");
    }

    let prs = reopen(&prs);
    assert_eq!(cell_texts(&table(&prs, 0, 0)), [["kept"]]);
    assert_eq!(frame_size(&prs, 0, 0), (Emu(INCH), Emu(INCH / 2)));
}

// Merged cells.

#[test]
fn a_saved_merge_splits_back_into_plain_cells_and_the_origin_keeps_the_gathered_text() {
    let mut prs = deck_with_table(3, 3);
    {
        let mut table = table_mut(&mut prs, 0, 0);
        number_cells(&mut table);
        table.cell_mut(0, 0).unwrap().merge_to(1, 1).unwrap();
    }
    let mut prs = reopen(&prs);
    table_mut(&mut prs, 0, 0)
        .cell_mut(0, 0)
        .unwrap()
        .split()
        .unwrap();

    let prs = reopen(&prs);
    let table = table(&prs, 0, 0);
    assert!((0..3).all(|row| (0..3).all(|column| is_plain(&table, row, column))));
    assert_eq!(
        cell_texts(&table),
        [
            ["r0c0\nr0c1\nr1c0\nr1c1", "", "r0c2"],
            ["", "", "r1c2"],
            ["r2c0", "r2c1", "r2c2"],
        ]
    );
    assert_eq!(column_widths(&table), [INCH; 3].map(Emu));
}

#[test]
fn overlapping_merges_and_splits_of_unmerged_cells_are_refused_and_change_nothing() {
    let mut prs = deck_with_table(3, 3);
    table_mut(&mut prs, 0, 0)
        .cell_mut(0, 0)
        .unwrap()
        .merge_to(1, 1)
        .unwrap();
    let before = prs.to_bytes().unwrap();
    {
        let mut table = table_mut(&mut prs, 0, 0);
        assert_refused(table.cell_mut(1, 1).unwrap().merge_to(2, 2), "merge cells");
        assert_refused(table.cell_mut(0, 2).unwrap().merge_to(0, 2), "merge cells");
        assert_refused(table.cell_mut(2, 0).unwrap().merge_to(3, 0), "merge cells");
        assert_refused(table.cell_mut(2, 2).unwrap().split(), "split cell");
        assert_refused(table.cell_mut(1, 1).unwrap().split(), "split cell");
    }
    assert!(
        prs.to_bytes().unwrap() == before,
        "a refused edit changed the deck"
    );
}

#[test]
fn a_row_or_column_inserted_inside_a_merge_extends_it() {
    let mut prs = deck_with_table(3, 3);
    {
        let mut table = table_mut(&mut prs, 0, 0);
        table.cell_mut(0, 0).unwrap().set_text("merged");
        table.cell_mut(0, 0).unwrap().merge_to(1, 1).unwrap();
        table.insert_row(1).unwrap();
        table.insert_column(1).unwrap();
    }

    let prs = reopen(&prs);
    let table = table(&prs, 0, 0);
    assert_eq!((table.row_count(), table.column_count()), (4, 4));
    let origin = table.cell(0, 0).unwrap();
    assert_eq!(origin.text(), "merged");
    assert_eq!((origin.span_height(), origin.span_width()), (3, 3));
    assert!(table.cell(2, 2).unwrap().is_spanned());
    assert!(is_plain(&table, 3, 1) && is_plain(&table, 1, 3));
}

#[test]
fn removing_the_first_row_or_column_of_a_merge_hands_it_on_with_its_text() {
    let mut prs = deck_with_table(3, 3);
    {
        let mut table = table_mut(&mut prs, 0, 0);
        table.cell_mut(0, 0).unwrap().set_text("merged");
        table.cell_mut(0, 0).unwrap().merge_to(1, 1).unwrap();
        table.remove_row(0).unwrap();
        let origin = table.cell(0, 0).unwrap();
        assert!(origin.is_merge_origin());
        assert_eq!((origin.span_height(), origin.span_width()), (1, 2));
        assert_eq!(origin.text(), "merged");
        table.remove_column(0).unwrap();
    }

    // One cell is left of the block: a plain cell with the text.
    let prs = reopen(&prs);
    let table = table(&prs, 0, 0);
    assert_eq!(table.cell(0, 0).unwrap().text(), "merged");
    assert!((0..2).all(|row| (0..2).all(|column| is_plain(&table, row, column))));
}

/// A 3x3 table whose top-left 2x2 block is merged as LibreOffice Impress
/// 26.2 saves it: the covered cells of the top row and of the left column
/// carry spans of 1, where python-pptx and this facade repeat the origin's
/// spans there.
fn table_merged_by_libreoffice() -> String {
    let cell = |attributes: &str, text: &str| {
        let paragraph = if text.is_empty() {
            "<a:p/>".to_owned()
        } else {
            format!("<a:p><a:r><a:t>{text}</a:t></a:r></a:p>")
        };
        format!(
            r#"<a:tc{attributes}><a:txBody><a:bodyPr/><a:lstStyle/>{paragraph}</a:txBody><a:tcPr/></a:tc>"#
        )
    };
    let rows = [
        [
            cell(r#" gridSpan="2" rowSpan="2""#, "merged"),
            cell(r#" hMerge="1" rowSpan="1""#, ""),
            cell("", "c"),
        ],
        [
            cell(r#" vMerge="1" gridSpan="1""#, ""),
            cell(r#" vMerge="1" hMerge="1""#, ""),
            cell("", "f"),
        ],
        [cell("", "g"), cell("", "h"), cell("", "i")],
    ];
    let rows: String = rows
        .iter()
        .map(|cells| format!(r#"<a:tr h="457200">{}</a:tr>"#, cells.concat()))
        .collect();
    format!(
        r#"<a:tbl xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:tblPr/><a:tblGrid>{}</a:tblGrid>{rows}</a:tbl>"#,
        r#"<a:gridCol w="914400"/>"#.repeat(3)
    )
}

#[test]
fn a_block_merged_by_libreoffice_can_be_split() {
    let mut prs = deck_with_table(3, 3);
    prs.replace_table_xml_at(ShapeOwner::Slide(0), &[0], &table_merged_by_libreoffice())
        .unwrap();
    let mut prs = reopen(&prs);
    {
        let table = table(&prs, 0, 0);
        let origin = table.cell(0, 0).unwrap();
        assert_eq!((origin.span_height(), origin.span_width()), (2, 2));
        assert!((0..2).all(|row| (0..2).all(
            |column| (row, column) == (0, 0) || table.cell(row, column).unwrap().is_spanned()
        )));
    }

    table_mut(&mut prs, 0, 0)
        .cell_mut(0, 0)
        .unwrap()
        .split()
        .unwrap();

    let prs = reopen(&prs);
    let table = table(&prs, 0, 0);
    assert!((0..3).all(|row| (0..3).all(|column| is_plain(&table, row, column))));
    assert_eq!(table.cell(0, 0).unwrap().text(), "merged");
}

#[test]
fn a_merged_block_whose_covered_cell_names_another_span_is_not_split() {
    // A covered cell may say 1, or repeat the origin's span in the top row
    // (rowSpan) and in the left column (gridSpan); anything else is refused,
    // an interior cell repeating either span included.
    for (covered, broken) in [
        (
            r#"<a:tc hMerge="1" rowSpan="1">"#,
            r#"<a:tc hMerge="1" rowSpan="3">"#,
        ),
        (
            r#"<a:tc vMerge="1" gridSpan="1">"#,
            r#"<a:tc vMerge="1" gridSpan="3">"#,
        ),
        (
            r#"<a:tc vMerge="1" hMerge="1">"#,
            r#"<a:tc vMerge="1" hMerge="1" rowSpan="2">"#,
        ),
        (
            r#"<a:tc vMerge="1" hMerge="1">"#,
            r#"<a:tc vMerge="1" hMerge="1" gridSpan="2">"#,
        ),
    ] {
        let table = table_merged_by_libreoffice().replacen(covered, broken, 1);
        let mut prs = deck_with_table(3, 3);
        prs.replace_table_xml_at(ShapeOwner::Slide(0), &[0], &table)
            .unwrap();
        let mut prs = reopen(&prs);
        let before = prs.to_bytes().unwrap();

        let refused = table_mut(&mut prs, 0, 0)
            .cell_mut(0, 0)
            .unwrap()
            .split()
            .unwrap_err();

        assert!(
            refused.to_string().contains("continuation rectangle"),
            "{broken}: {refused}"
        );
        assert_eq!(prs.to_bytes().unwrap(), before, "{broken}");
    }
}

// Table style.

#[test]
fn the_header_banding_and_edge_flags_round_trip() {
    let mut prs = deck_with_table(2, 2);
    {
        let mut table = table_mut(&mut prs, 0, 0);
        table.set_first_row(false);
        table.set_last_row(true);
        table.set_first_column(true);
        table.set_last_column(true);
        table.set_horizontal_banding(false);
        table.set_vertical_banding(true);
    }

    let prs = reopen(&prs);
    let table = table(&prs, 0, 0);
    assert!(!table.first_row() && !table.horizontal_banding());
    assert!(table.last_row() && table.first_column() && table.last_column());
    assert!(table.vertical_banding());
}

#[test]
fn a_table_style_applies_by_a_known_guid_only_and_comes_off_again() {
    let mut prs = deck_with_table(2, 2);
    table_mut(&mut prs, 0, 0)
        .set_style_id(Some(MEDIUM_STYLE_2_ACCENT_1.to_owned()))
        .unwrap();
    let mut prs = reopen(&prs);
    assert_eq!(table(&prs, 0, 0).style_id(), Some(MEDIUM_STYLE_2_ACCENT_1));

    {
        let mut table = table_mut(&mut prs, 0, 0);
        for refused in [
            // Neither built in nor defined in this deck.
            "{00000000-0000-0000-0000-000000000000}",
            GOOGLE_TABLE_STYLE,
            // Not a braced GUID.
            "Medium Style 2 - Accent 1",
            "5C22544A-7EE6-4342-B048-85BDC9FD1C3A",
        ] {
            assert_refused(
                table.set_style_id(Some(refused.to_owned())),
                "set table style",
            );
        }
        assert_eq!(table.style_id(), Some(MEDIUM_STYLE_2_ACCENT_1));
        table.set_style_id(None).unwrap();
    }
    let prs = reopen(&prs);
    assert_eq!(table(&prs, 0, 0).style_id(), None);
}

// The "Key figures" table of the corpus decks.

#[test]
fn editing_one_key_figures_cell_keeps_its_formatting_and_leaves_the_other_cells_alone() {
    let (slide, shape) = KEY_FIGURES;
    let owner = ShapeOwner::Slide(slide);
    let others: Vec<(usize, usize)> = (0..4)
        .flat_map(|row| (0..3).map(move |column| (row, column)))
        .filter(|&cell| cell != (0, 1))
        .collect();
    for producer in PRODUCERS {
        let mut prs = corpus_deck(&format!("{producer}/deck.pptx"));
        let look = first_run_look(&table(&prs, slide, shape), 0, 1);
        let style = table(&prs, slide, shape).style_id().map(str::to_owned);
        let cells_xml = |prs: &Presentation| -> Vec<Vec<u8>> {
            others
                .iter()
                .map(|&(row, column)| prs.cell_xml_at(owner, &[shape], row, column).unwrap())
                .collect()
        };
        let before = cells_xml(&prs);

        table_mut(&mut prs, slide, shape)
            .cell_mut(0, 1)
            .unwrap()
            .set_text("Q2 2026");

        let prs = reopen(&prs);
        let table = table(&prs, slide, shape);
        assert_eq!(table.cell(0, 1).unwrap().text(), "Q2 2026", "{producer}");
        assert_eq!(first_run_look(&table, 0, 1), look, "{producer}");
        assert_eq!(table.style_id(), style.as_deref(), "{producer}");
        assert!(
            cells_xml(&prs) == before,
            "{producer}: another cell changed"
        );
    }
}

#[test]
fn a_slide_imported_from_google_slides_brings_its_table_style() {
    let google = corpus_deck("google/deck.pptx");
    let mut prs = corpus_deck("powerpoint/deck.pptx");
    let (slide, shape) = KEY_FIGURES;
    assert_refused(
        table_mut(&mut prs, slide, shape).set_style_id(Some(GOOGLE_TABLE_STYLE.to_owned())),
        "set table style",
    );
    let title_only = layout(&prs, "Title Only");
    prs.import_slide(&google, slide, Some(title_only), None)
        .unwrap();

    let mut prs = reopen(&prs);
    let imported = table(&prs, prs.len() - 1, shape);
    assert_eq!(imported.style_id(), Some(GOOGLE_TABLE_STYLE));
    assert_eq!(cell_texts(&imported), KEY_FIGURES_TEXT);
    // The deck now defines Google's style, so its own table can take it.
    table_mut(&mut prs, slide, shape)
        .set_style_id(Some(GOOGLE_TABLE_STYLE.to_owned()))
        .unwrap();
}
