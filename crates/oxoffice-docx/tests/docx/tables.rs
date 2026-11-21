//! Body tables: adding and inserting them, reading and setting cell text,
//! rows and columns, merges and splits, nested tables, widths, borders,
//! shading, styles and header rows, on new documents and the corpus reports.

use std::collections::{HashMap, HashSet};

use oxoffice_docx::{
    Alignment, BorderStyle, CellBorderEdge, CellItemRef, CellTextDirection, CommentDate, Document,
    Length, RowHeight, StyleType, TableBorderEdge, TableCellMargins, TableConditionalFormatting,
    TableLayout, TableLook, TableWidth, TextCommentOptions, VMerge, VerticalAlignment,
};

use crate::support::{corpus_doc, doc_with, doc_with_body, reopen, texts};

const REPORTS: [&str; 4] = [
    "word/report.docx",
    "google/report.docx",
    "libreoffice/report.docx",
    "authored/report.docx",
];

/// The merged last row of the styled table of every corpus report.
const NOTE: &str = "Operating margin excludes one-off costs of the Lyon opening (212).";

const DATE: CommentDate = CommentDate::At("2026-11-20T00:00:00Z");

/// A new document holding one table with these cell texts, row by row.
fn table_doc(rows: &[&[&str]]) -> Document {
    let mut doc = Document::new();
    let columns = rows.first().map_or(0, |row| row.len());
    let mut table = doc.add_table(rows.len(), columns);
    for (row, cells) in rows.iter().enumerate() {
        for (cell, text) in cells.iter().enumerate() {
            if !text.is_empty() {
                table
                    .cell(row, cell)
                    .expect("the cell exists")
                    .set_text(text);
            }
        }
    }
    doc
}

/// The text of every cell of one table, row by row, each cell at its
/// position in the row (a merged row has fewer cells).
fn cell_texts(doc: &Document, table: usize) -> Vec<Vec<String>> {
    let table = doc.table(table).expect("the table exists");
    (0..table.row_count())
        .map(|row| {
            let row = table.row(row).expect("the row exists");
            (0..row.cell_count())
                .map(|cell| row.cell(cell).expect("the cell exists").text())
                .collect()
        })
        .collect()
}

/// The grid span of every cell of one row, 1 for a cell that is not merged.
fn spans(doc: &Document, table: usize, row: usize) -> Vec<u32> {
    let table = doc.table(table).expect("the table exists");
    let row = table.row(row).expect("the row exists");
    (0..row.cell_count())
        .map(|cell| {
            row.cell(cell)
                .expect("the cell exists")
                .grid_span()
                .unwrap_or(1)
        })
        .collect()
}

#[test]
fn a_new_table_keeps_its_size_and_cell_text_after_a_save() {
    let mut doc = table_doc(&[
        &["A1", "B1", "", "D1"],
        &["A2", "", "", ""],
        &["", "", "", "D3"],
    ]);
    let doc = reopen(&mut doc);
    assert_eq!(doc.table_count(), 1);
    let table = doc.table(0).unwrap();
    assert_eq!((table.row_count(), table.column_count()), (3, 4));
    assert_eq!(
        cell_texts(&doc, 0),
        [
            ["A1", "B1", "", "D1"],
            ["A2", "", "", ""],
            ["", "", "", "D3"]
        ]
    );
    // Word refuses a cell without a paragraph: an untouched cell keeps one.
    assert_eq!(table.cell(2, 1).unwrap().paragraph_count(), 1);
}

#[test]
fn add_and_insert_table_place_the_table_among_the_body_paragraphs() {
    let mut doc = doc_with(&["Before"]);
    doc.add_table(1, 1).cell(0, 0).unwrap().set_text("appended");
    doc.add_paragraph("After");
    doc.insert_table(1, 1, 1)
        .cell(0, 0)
        .unwrap()
        .set_text("inserted");
    let before = doc.to_bytes().unwrap();
    assert!(doc.try_insert_table(9, 1, 1).is_err());
    assert_eq!(doc.to_bytes().unwrap(), before);

    let doc = reopen(&mut doc);
    // Cell paragraphs are not body paragraphs.
    assert_eq!(texts(&doc), ["Before", "After"]);
    assert_eq!(doc.content_count(), 4);
    assert_eq!(doc.content_index_of_table(0), Some(1));
    assert_eq!(doc.content_index_of_table(1), Some(2));
    assert_eq!(cell_texts(&doc, 0), [["inserted"]]);
    assert_eq!(cell_texts(&doc, 1), [["appended"]]);
}

#[test]
fn document_text_puts_each_row_on_one_line_with_tab_ended_cells() {
    let mut doc = doc_with(&["Totals"]);
    {
        let mut table = doc.add_table(2, 2);
        table.cell(0, 0).unwrap().set_text("alpha");
        table.cell(0, 1).unwrap().set_text("beta");
        table.cell(1, 0).unwrap().set_text("gamma");
        table.cell(1, 1).unwrap().set_text("delta");
    }
    doc.add_paragraph("End");
    assert_eq!(doc.text(), "Totals\nalpha\tbeta\t\ngamma\tdelta\t\nEnd\n");

    // Find and replace reaches inside the cells.
    assert_eq!(doc.replace_all(&HashMap::from([("gamma", "GAMMA")])), 1);
    let doc = reopen(&mut doc);
    assert_eq!(cell_texts(&doc, 0)[1], ["GAMMA", "delta"]);
}

#[test]
fn cell_text_keeps_unicode_spaces_markup_and_line_breaks() {
    let long = "lorem ipsum ".repeat(2_000);
    let samples = [
        "Größe",
        "مرحبا بالعالم",
        "表格の中のテキスト",
        "👍🏽 done",
        "  padded  ",
        "a < b & \"c\" > 'd'",
        "line one\nline two\tafter a tab",
        long.as_str(),
    ];
    let mut doc = Document::new();
    {
        let mut table = doc.add_table(samples.len(), 1);
        for (row, sample) in samples.iter().enumerate() {
            table.cell(row, 0).unwrap().set_text(sample);
        }
    }
    let doc = reopen(&mut doc);
    let table = doc.table(0).unwrap();
    for (row, sample) in samples.iter().enumerate() {
        let cell = table.cell(row, 0).unwrap();
        assert_eq!(cell.text(), *sample);
        // A line feed is a line break inside the paragraph, not a new one.
        assert_eq!(cell.paragraph_count(), 1, "{sample:?}");
    }
}

#[test]
fn setting_cell_text_replaces_only_the_first_paragraph_and_can_clear_it() {
    let mut doc = Document::new();
    {
        let mut table = doc.add_table(1, 2);
        let mut cell = table.cell(0, 0).unwrap();
        cell.set_text("one");
        cell.add_paragraph("two");
        cell.set_text("uno");
        table.cell(0, 1).unwrap().set_text("to clear");
    }
    doc.try_set_cell_text(0, 0, 1, "").unwrap();

    let doc = reopen(&mut doc);
    let table = doc.table(0).unwrap();
    assert_eq!(table.cell(0, 0).unwrap().text(), "uno\ntwo");
    let cleared = table.cell(0, 1).unwrap();
    assert_eq!(cleared.text(), "");
    assert_eq!(cleared.paragraph_count(), 1);
}

#[test]
fn try_set_cell_text_refuses_bad_input_and_changes_nothing() {
    let mut doc = table_doc(&[&["kept"]]);
    let before = doc.to_bytes().unwrap();

    let error = doc.try_set_cell_text(0, 3, 0, "x").unwrap_err();
    assert!(error.to_string().contains("out of range"), "{error}");
    assert!(doc.try_set_cell_text(4, 0, 0, "x").is_err());
    let error = doc.try_set_cell_text(0, 0, 0, "bell \u{7}").unwrap_err();
    assert!(error.to_string().contains("U+0007"), "{error}");
    // The unchecked setter swallows the same refusal and leaves the cell.
    doc.table_mut(0)
        .unwrap()
        .cell(0, 0)
        .unwrap()
        .set_text("bell \u{7}");

    assert_eq!(doc.to_bytes().unwrap(), before);
}

#[test]
fn a_cell_holds_several_formatted_paragraphs() {
    let mut doc = Document::new();
    {
        let mut table = doc.add_table(1, 2);
        let mut cell = table.cell(0, 0).unwrap();
        cell.set_text("plain");
        let mut paragraph = cell.add_paragraph("");
        paragraph.set_alignment(Alignment::Center);
        paragraph.add_run("bold").bold(true);

        // A cell built paragraph by paragraph can drop its first empty one.
        let mut built = table.cell(0, 1).unwrap();
        built.add_paragraph("first");
        built.add_paragraph("second");
        built.remove_first_empty_paragraph();
    }
    let doc = reopen(&mut doc);
    let table = doc.table(0).unwrap();
    let cell = table.cell(0, 0).unwrap();
    assert_eq!(cell.text(), "plain\nbold");
    let paragraphs: Vec<_> = cell.paragraphs().map(|p| p.text()).collect();
    assert_eq!(paragraphs, ["plain", "bold"]);
    let second = cell.paragraph(1).unwrap();
    assert_eq!(second.alignment(), Some(Alignment::Center));
    assert!(second.runs().all(|run| run.is_bold()));
    assert_eq!(cell.paragraph(0).unwrap().alignment(), None);

    let built: Vec<_> = table
        .cell(0, 1)
        .unwrap()
        .paragraphs()
        .map(|p| p.text())
        .collect();
    assert_eq!(built, ["first", "second"]);
}

#[test]
fn add_row_copies_the_last_row_formatting_without_its_text_or_merge() {
    let mut doc = Document::new();
    {
        let mut table = doc.add_table(2, 2);
        table.merge_cells((0, 1), (1, 1)).unwrap();
        let mut cell = table.cell(1, 0).unwrap();
        cell.set_text("last");
        cell.set_shading_checked("D9D9D9").unwrap();
        cell.set_vertical_alignment(VerticalAlignment::Bottom);
        table
            .row(1)
            .unwrap()
            .set_height_checked(RowHeight::Exact(Length::pt(24.0)))
            .unwrap();
        assert_eq!(table.add_row().unwrap().cell_count(), 2);
    }
    assert_eq!(doc.add_empty_table_row(0).unwrap(), 3);

    let doc = reopen(&mut doc);
    let table = doc.table(0).unwrap();
    assert_eq!(table.row_count(), 4);
    for row in 2..4 {
        let cell = table.cell(row, 0).unwrap();
        assert_eq!(cell.text(), "");
        assert_eq!(cell.shading_fill(), Some("D9D9D9"));
        assert_eq!(cell.vertical_alignment(), Some(VerticalAlignment::Bottom));
        assert_eq!(
            table.row(row).unwrap().height(),
            Some(RowHeight::Exact(Length::pt(24.0)))
        );
        // A vertical merge stops at the rows it had.
        assert_eq!(table.cell(row, 1).unwrap().v_merge(), None);
    }
    assert_eq!(table.cell(1, 1).unwrap().v_merge(), Some(&VMerge::Continue));
}

#[test]
fn clone_table_row_copies_a_row_with_its_formatting_to_the_requested_place() {
    let mut doc = table_doc(&[&["Name", "Total"], &["", "12"]]);
    {
        let mut table = doc.table_mut(0).unwrap();
        table
            .cell(1, 0)
            .unwrap()
            .paragraph_mut(0)
            .unwrap()
            .add_run("North")
            .bold(true);
        table
            .cell(1, 1)
            .unwrap()
            .set_width_checked(Length::inches(2.0))
            .unwrap();
    }
    // A copy appended at the end, then a copy of the header above itself.
    assert_eq!(doc.clone_table_row(0, 1, 2).unwrap(), 2);
    assert_eq!(doc.clone_table_row(0, 0, 0).unwrap(), 0);

    let doc = reopen(&mut doc);
    assert_eq!(
        cell_texts(&doc, 0),
        [
            ["Name", "Total"],
            ["Name", "Total"],
            ["North", "12"],
            ["North", "12"]
        ]
    );
    let table = doc.table(0).unwrap();
    let copied = table.cell(3, 0).unwrap();
    let copied = copied.paragraph(0).unwrap();
    assert!(copied.runs().all(|run| run.is_bold()));
    assert_eq!(table.cell(3, 1).unwrap().width(), Some(Length::inches(2.0)));
}

#[test]
fn clone_table_row_refuses_what_would_break_the_table_and_changes_nothing() {
    let mut doc = table_doc(&[&["top"], &[""]]);
    doc.table_mut(0)
        .unwrap()
        .merge_cells((0, 0), (1, 0))
        .unwrap();
    let before = doc.to_bytes().unwrap();

    assert!(doc.clone_table_row(0, 2, 0).is_err());
    assert!(doc.clone_table_row(0, 0, 3).is_err());
    assert!(doc.clone_table_row(1, 0, 0).is_err());
    // A copy of the row that continues the merge cannot open the table.
    let error = doc.clone_table_row(0, 1, 0).unwrap_err();
    assert!(
        error.to_string().contains("no matching cell above"),
        "{error}"
    );

    assert_eq!(doc.to_bytes().unwrap(), before);
}

#[test]
fn remove_table_row_drops_one_row_and_keeps_at_least_one() {
    let mut doc = table_doc(&[&["first"], &["second"], &["third"]]);
    assert!(doc.remove_table_row(0, 1).unwrap());
    let mut doc = reopen(&mut doc);
    assert_eq!(cell_texts(&doc, 0), [["first"], ["third"]]);

    assert!(doc.remove_table_row(0, 1).unwrap());
    let before = doc.to_bytes().unwrap();
    let error = doc.remove_table_row(0, 0).unwrap_err();
    assert!(error.to_string().contains("at least one row"), "{error}");
    assert!(doc.remove_table_row(0, 5).is_err());
    assert_eq!(doc.to_bytes().unwrap(), before);
    assert_eq!(cell_texts(&doc, 0), [["first"]]);
}

#[test]
fn removing_the_first_row_of_a_vertical_merge_moves_its_start_down() {
    let mut doc = table_doc(&[&["top", "a"], &["", "b"], &["", "c"]]);
    doc.table_mut(0)
        .unwrap()
        .merge_cells((0, 0), (2, 0))
        .unwrap();
    assert!(doc.remove_table_row(0, 0).unwrap());

    let doc = reopen(&mut doc);
    let table = doc.table(0).unwrap();
    assert_eq!(table.row_count(), 2);
    assert_eq!(table.cell(0, 0).unwrap().v_merge(), Some(&VMerge::Restart));
    assert_eq!(table.cell(1, 0).unwrap().v_merge(), Some(&VMerge::Continue));
    assert_eq!(cell_texts(&doc, 0), [["", "b"], ["", "c"]]);
}

#[test]
fn removing_a_row_or_a_column_drops_the_comments_anchored_in_it() {
    let mut doc = table_doc(&[&["keep", "column note"], &["row note", ""]]);
    doc.add_comment_on_text(
        "row note",
        "Ann",
        "about the row",
        TextCommentOptions {
            date: DATE,
            ..Default::default()
        },
    )
    .unwrap();
    doc.add_comment_on_text(
        "column note",
        "Ann",
        "about the column",
        TextCommentOptions {
            date: DATE,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(doc.comments().len(), 2);

    doc.remove_table_row(0, 1).unwrap();
    assert_eq!(doc.comments().len(), 1);
    doc.delete_table_column(0, 1).unwrap();

    let doc = reopen(&mut doc);
    assert!(doc.comments().is_empty());
    assert_eq!(cell_texts(&doc, 0), [["keep"]]);
}

#[test]
fn inserting_and_deleting_a_column_change_every_row_and_the_table_width() {
    let mut doc = table_doc(&[&["a", "b"], &["c", "d"]]);
    assert_eq!(doc.table(0).unwrap().width(), Some(Length::twips(9_360)));
    doc.insert_table_column(0, 0, false).unwrap();
    doc.try_set_cell_text(0, 0, 1, "new").unwrap();

    let mut doc = reopen(&mut doc);
    assert_eq!(cell_texts(&doc, 0), [["a", "new", "b"], ["c", "", "d"]]);
    {
        let table = doc.table(0).unwrap();
        assert_eq!(table.column_count(), 3);
        // The new column takes the width of the column it was placed after.
        assert_eq!(table.width(), Some(Length::twips(9_360 + 4_680)));
    }

    doc.delete_table_column(0, 0).unwrap();
    let mut doc = reopen(&mut doc);
    assert_eq!(cell_texts(&doc, 0), [["new", "b"], ["", "d"]]);
    assert_eq!(doc.table(0).unwrap().width(), Some(Length::twips(9_360)));

    // Out-of-range columns and the last column left are refused.
    doc.delete_table_column(0, 0).unwrap();
    let before = doc.to_bytes().unwrap();
    let error = doc.delete_table_column(0, 0).unwrap_err();
    assert!(error.to_string().contains("at least one column"), "{error}");
    assert!(doc.delete_table_column(0, 1).is_err());
    assert!(doc.insert_table_column(0, 1, false).is_err());
    assert!(doc.insert_table_column(1, 0, true).is_err());
    assert_eq!(doc.to_bytes().unwrap(), before);
}

#[test]
fn column_edits_through_a_horizontal_merge_widen_or_narrow_the_span() {
    let mut doc = table_doc(&[&["wide", "", "right"], &["a", "b", "c"]]);
    doc.table_mut(0)
        .unwrap()
        .set_cell_grid_span_checked(0, 0, Some(2))
        .unwrap();

    // A column inserted inside the span widens it.
    doc.insert_table_column(0, 0, false).unwrap();
    assert_eq!(spans(&doc, 0, 0), [3, 1]);
    assert_eq!(
        cell_texts(&doc, 0),
        vec![vec!["wide", "right"], vec!["a", "", "b", "c"]]
    );

    // Deleting columns under the span narrows it and keeps its text.
    doc.delete_table_column(0, 0).unwrap();
    doc.delete_table_column(0, 0).unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(spans(&doc, 0, 0), [1, 1]);
    assert_eq!(cell_texts(&doc, 0), [["wide", "right"], ["b", "c"]]);
}

#[test]
fn merging_cells_across_a_row_writes_a_grid_span_and_narrowing_it_restores_the_cells() {
    let mut doc = table_doc(&[&["a", "b", ""], &["c", "d", "e"]]);
    assert_eq!(
        doc.table_mut(0)
            .unwrap()
            .merge_cells((0, 0), (0, 2))
            .unwrap(),
        (0, 0)
    );

    let mut doc = reopen(&mut doc);
    {
        let table = doc.table(0).unwrap();
        assert_eq!(table.row(0).unwrap().cell_count(), 1);
        let merged = table.cell(0, 0).unwrap();
        assert_eq!(merged.grid_span(), Some(3));
        // Each merged cell with text adds its paragraphs; an empty one adds none.
        assert_eq!(merged.text(), "a\nb");
        assert_eq!(merged.width(), Some(Length::twips(9_360)));
    }

    doc.table_mut(0)
        .unwrap()
        .set_cell_grid_span_checked(0, 0, None)
        .unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(spans(&doc, 0, 0), [1, 1, 1]);
    assert_eq!(cell_texts(&doc, 0)[0], ["a\nb", "", ""]);
    let table = doc.table(0).unwrap();
    for cell in 0..3 {
        assert_eq!(
            table.cell(0, cell).unwrap().width(),
            Some(Length::twips(3_120))
        );
    }
}

#[test]
fn merging_cells_down_a_column_writes_a_vertical_merge() {
    let mut doc = table_doc(&[&["top", "x"], &["middle", "y"], &["", "z"]]);
    doc.table_mut(0)
        .unwrap()
        .merge_cells((0, 0), (2, 0))
        .unwrap();

    let doc = reopen(&mut doc);
    let table = doc.table(0).unwrap();
    let merges: Vec<_> = (0..3)
        .map(|row| table.cell(row, 0).unwrap().v_merge().cloned())
        .collect();
    assert_eq!(
        merges,
        [
            Some(VMerge::Restart),
            Some(VMerge::Continue),
            Some(VMerge::Continue)
        ]
    );
    assert_eq!(
        cell_texts(&doc, 0),
        [["top\nmiddle", "x"], ["", "y"], ["", "z"]]
    );
}

#[test]
fn splitting_a_merged_block_restores_one_cell_per_grid_column_and_row() {
    let mut doc = table_doc(&[&["a", "", ""], &["", "b", ""], &["", "", ""]]);
    doc.table_mut(0)
        .unwrap()
        .merge_cells((0, 0), (1, 1))
        .unwrap();
    assert_eq!(spans(&doc, 0, 1), [2, 1]);
    // A merge splits from any of its cells, here by grid column in its second row.
    assert!(doc.split_table_cell(0, 1, 1).unwrap());

    let mut doc = reopen(&mut doc);
    assert_eq!(
        cell_texts(&doc, 0),
        [["a\nb", "", ""], ["", "", ""], ["", "", ""]]
    );
    for row in 0..3 {
        assert_eq!(spans(&doc, 0, row), [1, 1, 1]);
        assert_eq!(doc.table(0).unwrap().cell(row, 0).unwrap().v_merge(), None);
    }

    // A cell that is not merged stays as it is.
    let before = doc.to_bytes().unwrap();
    assert!(!doc.split_table_cell(0, 2, 2).unwrap());
    assert_eq!(doc.table_mut(0).unwrap().split_cell(2, 2).unwrap(), 1);
    assert_eq!(doc.to_bytes().unwrap(), before);
}

#[test]
fn merges_refuse_to_swallow_text_overrun_the_grid_or_continue_nothing() {
    let mut doc = table_doc(&[&["", "full", ""], &["", "", ""]]);
    doc.table_mut(0)
        .unwrap()
        .merge_cells((0, 2), (1, 2))
        .unwrap();
    let before = doc.to_bytes().unwrap();
    {
        let mut table = doc.table_mut(0).unwrap();
        let error = table.set_cell_grid_span_checked(0, 0, Some(2)).unwrap_err();
        assert!(error.to_string().contains("nonempty cell"), "{error}");
        let error = table.set_cell_grid_span_checked(1, 0, Some(4)).unwrap_err();
        assert!(
            error.to_string().contains("exceeds the row grid"),
            "{error}"
        );
        let error = table
            .set_cell_vertical_merge(0, 0, Some(VMerge::Continue))
            .unwrap_err();
        assert!(
            error.to_string().contains("no matching cell above"),
            "{error}"
        );
        // The last cell already belongs to a vertical merge.
        let error = table.merge_cells((0, 1), (0, 2)).unwrap_err();
        assert!(error.to_string().contains("existing merge"), "{error}");
    }
    assert_eq!(doc.to_bytes().unwrap(), before);
}

#[test]
fn a_table_nested_in_a_cell_survives_a_save_and_its_cell_text_can_change() {
    let mut doc = Document::new();
    {
        let mut table = doc.add_table(1, 2);
        let mut cell = table.cell(0, 0).unwrap();
        cell.set_text("outer");
        let mut nested = cell.add_table_checked(2, 1).unwrap();
        nested.cell(1, 0).unwrap().set_text("inner");
        assert!(table.cell(0, 1).unwrap().add_table_checked(0, 2).is_err());
    }
    doc.try_set_cell_text(0, 0, 0, "changed").unwrap();

    let doc = reopen(&mut doc);
    let table = doc.table(0).unwrap();
    let cell = table.cell(0, 0).unwrap();
    let items: Vec<String> = cell
        .items()
        .map(|item| match item {
            CellItemRef::Paragraph(paragraph) => format!("paragraph {}", paragraph.text()),
            CellItemRef::Table(nested) => format!(
                "table {}x{} {}",
                nested.row_count(),
                nested.column_count(),
                nested.cell(1, 0).unwrap().text()
            ),
            _ => "other".to_owned(),
        })
        .collect();
    // Word refuses a cell that ends with a table, so a paragraph follows it.
    assert_eq!(
        items,
        ["paragraph changed", "table 2x1 inner", "paragraph "]
    );
    // The text of the cell leaves the nested table out.
    assert_eq!(cell.text(), "changed\n");
}

#[test]
fn nested_tables_are_not_counted_among_the_document_tables() {
    let mut doc = Document::new();
    doc.add_table(1, 1)
        .cell(0, 0)
        .unwrap()
        .add_table(1, 1)
        .cell(0, 0)
        .unwrap()
        .set_text("nested");
    doc.add_table(1, 1).cell(0, 0).unwrap().set_text("second");
    assert_eq!(doc.table_count(), 2);
    // Table indexes count body tables only: index 1 is the second one.
    assert_eq!(doc.clone_table_row(1, 0, 1).unwrap(), 1);

    let doc = reopen(&mut doc);
    assert_eq!(doc.table_count(), 2);
    assert_eq!(doc.table(0).unwrap().row_count(), 1);
    assert_eq!(cell_texts(&doc, 1), [["second"], ["second"]]);
}

#[test]
fn table_style_alignment_width_layout_and_caption_survive_a_save() {
    let mut doc = Document::new();
    // A new document defines Table Grid, so a table can name it at once.
    assert_eq!(
        doc.style("TableGrid")
            .and_then(|style| style.name().map(str::to_owned))
            .as_deref(),
        Some("Table Grid")
    );
    {
        let mut table = doc.add_table(2, 2);
        table.set_style("TableGrid");
        table.set_alignment(Alignment::Center);
        table
            .set_width_mode(TableWidth::Fixed(Length::inches(5.0)))
            .unwrap();
        table.set_layout(TableLayout::Fixed);
        table.set_indent_checked(Length::twips(-120)).unwrap();
        table
            .set_cell_margins_checked(
                Length::pt(0.0),
                Length::pt(5.0),
                Length::pt(0.0),
                Length::pt(5.0),
            )
            .unwrap();
        table.set_caption(Some("Quarterly results")).unwrap();
        table
            .set_description(Some("Revenue and costs by quarter"))
            .unwrap();
        assert!(table.set_caption(Some("  ")).is_err());
        assert!(table.set_width_mode(TableWidth::Percentage(150.0)).is_err());
    }
    doc.add_table(1, 1)
        .set_width_mode(TableWidth::Percentage(50.0))
        .unwrap();

    let doc = reopen(&mut doc);
    let table = doc.table(0).unwrap();
    assert_eq!(table.style_id(), Some("TableGrid"));
    assert_eq!(table.alignment(), Some(Alignment::Center));
    assert_eq!(
        table.width_mode(),
        Some(TableWidth::Fixed(Length::inches(5.0)))
    );
    assert_eq!(table.layout(), Some(TableLayout::Fixed));
    assert_eq!(table.indent(), Some(Length::twips(-120)));
    assert_eq!(
        table.cell_margins(),
        Some(TableCellMargins {
            top: Some(Length::pt(0.0)),
            right: Some(Length::pt(5.0)),
            bottom: Some(Length::pt(0.0)),
            left: Some(Length::pt(5.0)),
        })
    );
    assert_eq!(table.caption(), Some("Quarterly results"));
    assert_eq!(table.description(), Some("Revenue and costs by quarter"));
    assert_eq!(
        doc.table(1).unwrap().width_mode(),
        Some(TableWidth::Percentage(50.0))
    );
}

#[test]
fn a_table_width_of_33_point_3_percent_reads_back_as_written() {
    let mut doc = Document::new();
    doc.add_table(1, 1)
        .set_width_mode(TableWidth::Percentage(33.3))
        .unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(
        doc.table(0).unwrap().width_mode(),
        Some(TableWidth::Percentage(33.3))
    );
}

#[test]
fn table_percentages_round_to_the_nearest_fiftieth_through_both_setters() {
    for (percent, stored) in [
        (33.3, 33.3),
        (66.7, 66.7),
        (12.5, 12.5),
        (99.99, 100.0),
        // Half a fiftieth rounds up, below it rounds down.
        (0.01, 0.02),
        (0.004, 0.0),
    ] {
        let mut doc = Document::new();
        doc.add_table(1, 1)
            .set_width_mode(TableWidth::Percentage(percent))
            .unwrap();
        doc.add_table(1, 1).set_width_pct(percent);
        let doc = reopen(&mut doc);
        for index in 0..2 {
            assert_eq!(
                doc.table(index).unwrap().width_mode(),
                Some(TableWidth::Percentage(stored)),
                "{percent} through setter {index}"
            );
        }
    }
}

#[test]
fn a_cell_reads_its_tracked_insertions_as_its_paragraphs_do() {
    let doc = doc_with_body(concat!(
        r#"<w:tbl><w:tblGrid><w:gridCol w:w="4000"/></w:tblGrid><w:tr><w:tc><w:p>"#,
        r#"<w:r><w:t>A</w:t></w:r>"#,
        r#"<w:ins w:id="1" w:author="Ana" w:date="2026-01-01T00:00:00Z"><w:r><w:t>B</w:t></w:r></w:ins>"#,
        r#"<w:del w:id="2" w:author="Ana" w:date="2026-01-01T00:00:00Z"><w:r><w:delText>C</w:delText></w:r></w:del>"#,
        r#"</w:p></w:tc></w:tr></w:tbl>"#,
    ));
    let table = doc.table(0).unwrap();
    let cell = table.cell(0, 0).unwrap();
    assert_eq!(cell.paragraph(0).unwrap().text(), "AB");
    assert_eq!(cell.text(), "AB", "the cell reads its paragraph's text");
}

#[test]
fn ensure_builtin_table_style_adds_light_shading_accent_1_once() {
    let mut doc = Document::new();
    let before = doc.to_bytes().unwrap();
    // A style the facade does not carry is not invented.
    assert!(!doc.ensure_builtin_table_style("Medium Grid 1").unwrap());
    assert_eq!(doc.to_bytes().unwrap(), before);

    assert!(
        doc.ensure_builtin_table_style("Light Shading Accent 1")
            .unwrap()
    );
    assert!(
        !doc.ensure_builtin_table_style("LightShading-Accent1")
            .unwrap()
    );
    doc.add_table(1, 1).set_style("LightShading-Accent1");
    let doc = reopen(&mut doc);
    assert_eq!(
        doc.table(0).unwrap().style_id(),
        Some("LightShading-Accent1")
    );
    let style = doc
        .style("LightShading-Accent1")
        .expect("the style is defined");
    assert_eq!(style.name(), Some("Light Shading Accent 1"));
    assert_eq!(style.style_type(), StyleType::Table);

    // French Word saves it as `Trameclaire-Accent1`: found by name, not added twice.
    let mut word = corpus_doc("word/report.docx");
    let count = word.styles().len();
    assert!(
        !word
            .ensure_builtin_table_style("Light Shading Accent 1")
            .unwrap()
    );
    assert_eq!(word.styles().len(), count);
}

#[test]
fn table_borders_read_back_edge_by_edge_and_bad_values_are_refused() {
    let mut doc = Document::new();
    doc.add_table(2, 2);
    assert!(doc.table(0).unwrap().border(TableBorderEdge::Top).is_none());
    {
        let mut table = doc.table_mut(0).unwrap();
        table
            .set_all_borders_checked(BorderStyle::Single, 4, "000000")
            .unwrap();
        table
            .set_border_checked(
                TableBorderEdge::InsideVertical,
                BorderStyle::Dashed,
                8,
                "FF0000",
            )
            .unwrap();
    }
    let before = doc.to_bytes().unwrap();
    {
        let mut table = doc.table_mut(0).unwrap();
        let error = table
            .set_border_checked(TableBorderEdge::Top, BorderStyle::Single, 4, "red")
            .unwrap_err();
        assert!(
            error.to_string().contains("six hexadecimal digits"),
            "{error}"
        );
        let error = table
            .set_border_checked(TableBorderEdge::Top, BorderStyle::Single, 97, "000000")
            .unwrap_err();
        assert!(error.to_string().contains("1 through 96"), "{error}");
    }
    assert_eq!(doc.to_bytes().unwrap(), before);

    let doc = reopen(&mut doc);
    let table = doc.table(0).unwrap();
    let edge = |position| {
        table.border(position).map(|border| {
            (
                border.style(),
                border.size_eighths_pt(),
                border.color().map(str::to_owned),
            )
        })
    };
    for position in [
        TableBorderEdge::Top,
        TableBorderEdge::Bottom,
        TableBorderEdge::Left,
        TableBorderEdge::Right,
        TableBorderEdge::InsideHorizontal,
    ] {
        assert_eq!(
            edge(position),
            Some(("single", Some(4), Some("000000".to_owned()))),
            "{position:?}"
        );
    }
    assert_eq!(
        edge(TableBorderEdge::InsideVertical),
        Some(("dashed", Some(8), Some("FF0000".to_owned())))
    );
}

#[test]
fn cell_shading_borders_margins_and_alignment_survive_a_save() {
    let mut doc = Document::new();
    {
        let mut table = doc.add_table(1, 2);
        let mut cell = table.cell(0, 1).unwrap();
        cell.set_shading_checked("D9D9D9").unwrap();
        cell.set_border_checked(CellBorderEdge::Bottom, BorderStyle::Double, 6, "auto")
            .unwrap();
        cell.set_margins_checked(
            Length::pt(1.0),
            Length::pt(2.0),
            Length::pt(3.0),
            Length::pt(4.0),
        )
        .unwrap();
        cell.set_vertical_alignment(VerticalAlignment::Center);
        cell.set_text_direction(Some(CellTextDirection::BottomToTopLeftToRight));
        cell.set_no_wrap();
        cell.set_width_checked(Length::inches(2.0)).unwrap();
        let error = cell.set_shading_checked("yellow").unwrap_err();
        assert!(
            error.to_string().contains("six hexadecimal digits"),
            "{error}"
        );
    }

    let doc = reopen(&mut doc);
    let table = doc.table(0).unwrap();
    let cell = table.cell(0, 1).unwrap();
    assert_eq!(cell.shading_fill(), Some("D9D9D9"));
    let bottom = cell.border(CellBorderEdge::Bottom).unwrap();
    assert_eq!(
        (bottom.style(), bottom.size_eighths_pt(), bottom.color()),
        ("double", Some(6), Some("auto"))
    );
    assert!(cell.border(CellBorderEdge::Top).is_none());
    assert_eq!(
        cell.margins(),
        Some(TableCellMargins {
            top: Some(Length::pt(1.0)),
            right: Some(Length::pt(2.0)),
            bottom: Some(Length::pt(3.0)),
            left: Some(Length::pt(4.0)),
        })
    );
    assert_eq!(cell.vertical_alignment(), Some(VerticalAlignment::Center));
    assert_eq!(
        cell.text_direction(),
        Some(CellTextDirection::BottomToTopLeftToRight)
    );
    assert_eq!(cell.no_wrap_value(), Some(true));
    assert_eq!(cell.width(), Some(Length::inches(2.0)));

    let neighbour = table.cell(0, 0).unwrap();
    assert_eq!(neighbour.shading_fill(), None);
    assert_eq!(neighbour.vertical_alignment(), None);
}

#[test]
fn table_look_selects_the_header_row_and_banding_and_can_be_cleared() {
    let look = TableLook {
        first_row: true,
        horizontal_banding: true,
        ..TableLook::default()
    };
    let mut doc = Document::new();
    {
        let mut table = doc.add_table(3, 2);
        table.set_style("TableGrid");
        table.set_look(look);
        table.set_row_band_size(2).unwrap();
        assert!(table.set_row_band_size(0).is_err());
        table
            .row(0)
            .unwrap()
            .set_conditional_formatting(TableConditionalFormatting {
                first_row: true,
                ..TableConditionalFormatting::default()
            });
    }

    let mut doc = reopen(&mut doc);
    {
        let table = doc.table(0).unwrap();
        assert_eq!(table.look(), Some(look));
        assert_eq!(table.row_band_size(), Some(2));
        let regions = table.row(0).unwrap().conditional_formatting().unwrap();
        assert!(regions.first_row && !regions.last_row);
        assert_eq!(table.row(1).unwrap().conditional_formatting(), None);
    }

    doc.table_mut(0).unwrap().clear_look();
    let doc = reopen(&mut doc);
    assert_eq!(doc.table(0).unwrap().look(), None);
}

#[test]
fn repeated_header_rows_and_row_heights_survive_a_save() {
    let mut doc = table_doc(&[&["Region", "Revenue"], &["", "EUR k"], &["North", "1,402"]]);
    {
        let mut table = doc.table_mut(0).unwrap();
        // Word repeats these rows at the top of every page the table runs onto.
        table.row(0).unwrap().set_header();
        table.row(1).unwrap().set_header();
        table
            .row(0)
            .unwrap()
            .set_height_checked(RowHeight::Exact(Length::pt(30.0)))
            .unwrap();
        let mut last = table.row(2).unwrap();
        last.set_height_checked(RowHeight::AtLeast(Length::pt(20.0)))
            .unwrap();
        last.set_cant_split();
        let error = last
            .set_height_checked(RowHeight::Exact(Length::pt(-1.0)))
            .unwrap_err();
        assert!(error.to_string().contains("cannot be negative"), "{error}");
    }

    let doc = reopen(&mut doc);
    let table = doc.table(0).unwrap();
    let headers: Vec<_> = (0..3)
        .map(|row| table.row(row).unwrap().is_header())
        .collect();
    assert_eq!(headers, [true, true, false]);
    let last = table.row(2).unwrap();
    assert_eq!(last.header_value(), None);
    assert_eq!(
        table.row(0).unwrap().height(),
        Some(RowHeight::Exact(Length::pt(30.0)))
    );
    assert_eq!(last.height(), Some(RowHeight::AtLeast(Length::pt(20.0))));
    assert_eq!(last.cant_split_value(), Some(true));
    assert_eq!(table.row(1).unwrap().height(), None);
}

#[test]
fn column_widths_keep_the_grid_the_cells_and_the_table_width_in_step() {
    let mut doc = Document::new();
    {
        let mut table = doc.add_table(2, 3);
        table.set_cell_grid_span_checked(0, 0, Some(2)).unwrap();
        table
            .set_grid_widths(&[
                Length::inches(1.0),
                Length::inches(2.0),
                Length::inches(3.0),
            ])
            .unwrap();
        assert!(table.set_column_width(2, Length::inches(1.5)));
        assert!(!table.set_column_width(3, Length::inches(1.0)));
        let error = table.set_grid_widths(&[Length::inches(1.0)]).unwrap_err();
        assert!(error.to_string().contains("exactly 3"), "{error}");
    }

    let doc = reopen(&mut doc);
    let table = doc.table(0).unwrap();
    assert_eq!(
        table.grid_widths(),
        [
            Length::inches(1.0),
            Length::inches(2.0),
            Length::inches(1.5)
        ]
    );
    assert_eq!(table.width(), Some(Length::inches(4.5)));
    // A merged cell takes the sum of the grid columns it covers.
    assert_eq!(table.cell(0, 0).unwrap().width(), Some(Length::inches(3.0)));
    let row: Vec<_> = (0..3)
        .map(|cell| table.cell(1, cell).unwrap().width())
        .collect();
    assert_eq!(
        row,
        [
            Some(Length::inches(1.0)),
            Some(Length::inches(2.0)),
            Some(Length::inches(1.5))
        ]
    );
}

#[test]
fn every_report_has_its_styled_table_with_a_merged_last_row() {
    for name in REPORTS {
        let doc = corpus_doc(name);
        assert_eq!(doc.table_count(), 2, "{name}");
        let table = doc.table(0).unwrap();
        assert_eq!((table.row_count(), table.column_count()), (6, 4), "{name}");
        let texts = cell_texts(&doc, 0);
        assert_eq!(texts[0], ["Indicator", "Q2", "Q3", "Change"], "{name}");
        assert_eq!(
            texts[4],
            ["Operating margin", "867", "1,122", "+29.4 %"],
            "{name}"
        );
        // Cells are addressed by position in the row: the note row has one.
        assert_eq!(texts[5], [NOTE], "{name}");
        assert_eq!(table.cell(5, 0).unwrap().grid_span(), Some(4), "{name}");
        assert!(table.cell(5, 1).is_none(), "{name}");
        assert_eq!(table.alignment(), Some(Alignment::Center), "{name}");
        assert_eq!(
            table.look().map(|look| look.first_row),
            Some(true),
            "{name}"
        );
        let style = table
            .style_id()
            .and_then(|id| doc.style(id))
            .unwrap_or_else(|| panic!("{name}: the table style is defined"));
        // Google writes unnamed styles of its own, the others the built-in one.
        if !name.starts_with("google") {
            assert_eq!(style.name(), Some("Light Grid Accent 1"), "{name}");
        }
    }
}

#[test]
fn every_report_has_its_table_grid_table_in_the_landscape_section() {
    for name in REPORTS {
        let doc = corpus_doc(name);
        let table = doc.table(1).unwrap();
        assert_eq!((table.row_count(), table.column_count()), (5, 5), "{name}");
        let texts = cell_texts(&doc, 1);
        assert_eq!(
            texts[0],
            ["Region", "Customers", "Revenue", "Churn", "Tickets"],
            "{name}"
        );
        assert_eq!(
            texts[4],
            ["West", "1,010", "1,128", "2.2 %", "4,420"],
            "{name}"
        );
        if !name.starts_with("google") {
            let style = table.style_id().and_then(|id| doc.style(id)).unwrap();
            assert_eq!(style.name(), Some("Table Grid"), "{name}");
        }
        // No facade reader maps a body item to its section, so the order is
        // read from the part: the table follows the first section break, and
        // the last section is landscape.
        let xml = doc.part_xml("word/document.xml").unwrap();
        let first_break = xml.find("<w:sectPr").unwrap();
        let second_table = xml.match_indices("<w:tbl>").nth(1).unwrap().0;
        assert!(first_break < second_table, "{name}");
        let (width, height) = doc.sections().last().unwrap().page_size().unwrap();
        assert!(width.to_emu() > height.to_emu(), "{name}");
    }
}

#[test]
fn editing_a_cell_of_the_word_report_keeps_the_rest_of_the_tables() {
    let mut doc = corpus_doc("word/report.docx");
    let mut expected = cell_texts(&doc, 0);
    let second = cell_texts(&doc, 1);
    doc.try_set_cell_text(0, 1, 1, "3,800").unwrap();

    let doc = reopen(&mut doc);
    expected[1][1] = "3,800".to_owned();
    assert_eq!(cell_texts(&doc, 0), expected);
    assert_eq!(cell_texts(&doc, 1), second);
    let table = doc.table(0).unwrap();
    assert_eq!(table.style_id(), Some("Grilleclaire-Accent1"));
    assert_eq!(
        table.look(),
        Some(TableLook {
            first_row: true,
            first_column: true,
            horizontal_banding: true,
            ..TableLook::default()
        })
    );
    assert_eq!(table.cell(5, 0).unwrap().grid_span(), Some(4));
    // Word's conditional formatting marks stay on the header row.
    let regions = table.row(0).unwrap().conditional_formatting().unwrap();
    assert!(regions.first_row);
}

#[test]
fn rows_added_to_the_word_report_keep_its_paragraph_ids_unique() {
    let mut doc = corpus_doc("word/report.docx");
    assert_eq!(doc.clone_table_row(0, 1, 5).unwrap(), 5);
    assert_eq!(doc.add_empty_table_row(1).unwrap(), 5);

    let doc = reopen(&mut doc);
    let first = cell_texts(&doc, 0);
    assert_eq!(first.len(), 7);
    // The copy sits before the note row, which stays last.
    assert_eq!(first[5], first[1]);
    assert_eq!(first[6], [NOTE]);
    assert_eq!(cell_texts(&doc, 1)[5], ["", "", "", "", ""]);
    // Word gives every paragraph a `w14:paraId`; a copy must not repeat one.
    let xml = doc.part_xml("word/document.xml").unwrap();
    let ids: Vec<&str> = xml
        .split("w14:paraId=\"")
        .skip(1)
        .map(|rest| &rest[..8])
        .collect();
    let unique: HashSet<_> = ids.iter().collect();
    assert!(!ids.is_empty());
    assert_eq!(unique.len(), ids.len());
}

#[test]
fn columns_of_the_word_report_change_around_its_merged_last_row() {
    let mut doc = corpus_doc("word/report.docx");
    doc.delete_table_column(0, 3).unwrap();
    // The note row is split by any grid column it covers.
    assert!(doc.split_table_cell(0, 5, 2).unwrap());

    let doc = reopen(&mut doc);
    let texts = cell_texts(&doc, 0);
    assert_eq!(texts[0], ["Indicator", "Q2", "Q3"]);
    assert_eq!(texts[4], ["Operating margin", "867", "1,122"]);
    assert_eq!(texts[5], [NOTE, "", ""]);
    assert_eq!(spans(&doc, 0, 5), [1, 1, 1]);
    assert_eq!(doc.table(0).unwrap().column_count(), 3);
}

#[test]
fn rows_added_to_google_and_libreoffice_tables_keep_their_formatting() {
    // Google marks every row as explicitly not a header; so does a new row.
    let mut google = corpus_doc("google/report.docx");
    assert_eq!(
        google.table(1).unwrap().row(0).unwrap().header_value(),
        Some(false)
    );
    assert_eq!(google.add_empty_table_row(1).unwrap(), 5);
    let google = reopen(&mut google);
    let row = google.table(1).unwrap();
    let row = row.row(5).unwrap();
    assert_eq!(row.header_value(), Some(false));
    assert!(!row.is_header());
    // Google writes the table width as `9744.0`, which reads as 9744 twips.
    assert_eq!(google.table(0).unwrap().width(), Some(Length::twips(9_744)));

    // LibreOffice shades the note row; a row added after it keeps the fill.
    let mut libre = corpus_doc("libreoffice/report.docx");
    libre.table_mut(0).unwrap().add_row().unwrap();
    let libre = reopen(&mut libre);
    let table = libre.table(0).unwrap();
    assert_eq!(table.row_count(), 7);
    let added = table.cell(6, 0).unwrap();
    assert_eq!(added.text(), "");
    assert_eq!(added.shading_fill(), Some("D3DFEE"));
    assert_eq!(added.grid_span(), Some(4));
}

#[test]
fn a_property_edit_in_place_changes_the_table_and_keeps_its_cells() {
    for name in REPORTS {
        let mut doc = corpus_doc(name);
        let before = cell_texts(&doc, 0);
        let edited = doc
            .edit_table_properties_in_place(0, |table| {
                table.set_alignment(Alignment::Right);
                table
                    .row(1)
                    .expect("the row exists")
                    .set_header_value(Some(true));
                Ok(())
            })
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(edited, Some(()), "{name}");
        let reopened = reopen(&mut doc);
        let table = reopened.table(0).expect("the table exists");
        assert_eq!(table.alignment(), Some(Alignment::Right), "{name}");
        assert_eq!(table.row(1).unwrap().header_value(), Some(true), "{name}");
        assert_eq!(cell_texts(&reopened, 0), before, "{name}");
    }
}

/// A one-cell table whose cell holds these properties and this paragraph.
fn one_cell_table(properties: &str, paragraph: &str) -> String {
    format!(
        r#"<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:tc>{properties}{paragraph}</w:tc></w:tr></w:tbl><w:p/>"#
    )
}

#[test]
fn google_docs_and_libreoffice_reports_render_as_templates() {
    // Google Docs writes an empty `w:tcPr` in every table cell, LibreOffice
    // an empty `w:trPr` in every row.
    for name in ["google/report.docx", "libreoffice/report.docx"] {
        let mut doc = corpus_doc(name);
        let before = cell_texts(&doc, 0);
        assert_eq!(
            doc.render_template(&serde_json::json!({}))
                .unwrap_or_else(|e| panic!("{name}: {e}")),
            0,
            "{name}"
        );
        let doc = reopen(&mut doc);
        assert_eq!(cell_texts(&doc, 0), before, "{name}");
    }
}

#[test]
fn a_cell_property_is_edited_in_place_unless_it_is_the_cells_first() {
    // Word writes the width of every cell, Google Docs an empty `w:tcPr`.
    let mut word = corpus_doc("word/report.docx");
    let edited = word
        .edit_table_properties_in_place(0, |table| {
            table
                .cell(1, 0)
                .expect("the cell")
                .set_shading_checked("FFFF00")
        })
        .expect("the edit runs");
    assert_eq!(edited, Some(()));
    let word = reopen(&mut word);
    assert_eq!(
        word.table(0).unwrap().cell(1, 0).unwrap().shading_fill(),
        Some("FFFF00")
    );

    // Google's empty `w:tcPr` is written already, so a property fills it.
    let mut google = corpus_doc("google/report.docx");
    let edited = google
        .edit_table_properties_in_place(0, |table| {
            table
                .cell(1, 0)
                .expect("the cell")
                .set_shading_checked("FFFF00")
        })
        .expect("the edit runs");
    assert_eq!(edited, Some(()));
    let google = reopen(&mut google);
    assert_eq!(
        google.table(0).unwrap().cell(1, 0).unwrap().shading_fill(),
        Some("FFFF00")
    );

    // A first property writes a `w:tcPr`, which moves the cell's paragraphs.
    let mut bare = doc_with_body(&one_cell_table("", "<w:p/>"));
    let before = bare.to_bytes().expect("the document saves");
    let edited = bare
        .edit_table_properties_in_place(0, |table| {
            table
                .cell(0, 0)
                .expect("the cell")
                .set_shading_checked("FFFF00")
        })
        .expect("the edit runs");
    assert_eq!(edited, None);
    assert_eq!(bare.to_bytes().expect("the document saves"), before);
}

#[test]
fn an_in_place_table_edit_that_moves_content_is_undone_and_declined() {
    let mut doc = corpus_doc("word/report.docx");
    let before = doc.to_bytes().expect("the report saves");
    let edited = doc
        .edit_table_properties_in_place(0, |table| {
            table.set_alignment(Alignment::Right);
            table.add_row().map(|_| ())
        })
        .expect("the edit runs");
    assert_eq!(edited, None, "a new row is content, not a property");
    let edited = doc
        .edit_table_properties_in_place(0, |table| {
            table.set_alignment(Alignment::Right);
            table
                .cell(1, 0)
                .expect("the cell exists")
                .set_text("changed");
            Ok(())
        })
        .expect("the edit runs");
    assert_eq!(edited, None, "cell text is content, not a property");
    assert_eq!(doc.to_bytes().expect("the report saves"), before);
}

#[test]
fn a_failed_or_panicking_in_place_table_edit_leaves_the_table_as_it_was() {
    let mut doc = corpus_doc("word/report.docx");
    let before = doc.to_bytes().expect("the report saves");
    let failed = doc.edit_table_properties_in_place(0, |table| {
        table.set_alignment(Alignment::Right);
        table
            .cell(1, 0)
            .expect("the cell exists")
            .set_shading_checked("not a colour")
    });
    assert!(failed.is_err(), "a bad shading colour is refused");
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        doc.edit_table_properties_in_place(0, |table| -> oxoffice_docx::Result<()> {
            table.set_alignment(Alignment::Right);
            panic!("an edit that panics half way");
        })
    }));
    assert!(panicked.is_err(), "the panic reaches the caller");
    assert!(doc.edit_table_properties_in_place(99, |_| Ok(())).is_err());
    assert_eq!(doc.to_bytes().expect("the report saves"), before);
}

#[test]
fn a_template_tag_in_a_cell_with_empty_properties_renders() {
    let mut doc = doc_with_body(&one_cell_table(
        "<w:tcPr/>",
        "<w:p><w:r><w:t>{{ v }}</w:t></w:r></w:p>",
    ));
    assert_eq!(
        doc.render_template(&serde_json::json!({ "v": "V" }))
            .unwrap(),
        1
    );
    let doc = reopen(&mut doc);
    assert_eq!(cell_texts(&doc, 0), [["V"]]);
    let xml = doc.part_xml("word/document.xml").unwrap();
    assert!(xml.contains("<w:tc><w:tcPr/><w:p"), "{xml}");
}

#[test]
fn a_save_keeps_empty_cell_properties() {
    for properties in ["<w:tcPr/>", "<w:tcPr></w:tcPr>"] {
        let body = one_cell_table(properties, "<w:p/>");
        let doc = reopen(&mut doc_with_body(&body));
        let xml = doc.part_xml("word/document.xml").unwrap();
        assert!(
            xml.contains(&format!("<w:tc>{properties}<w:p/></w:tc>")),
            "{xml}"
        );

        // An edit of the cell writes it again from the model.
        let mut doc = doc_with_body(&body);
        doc.try_set_cell_text(0, 0, 0, "x").unwrap();
        let doc = reopen(&mut doc);
        let xml = doc.part_xml("word/document.xml").unwrap();
        assert!(xml.contains("<w:tc><w:tcPr/><w:p>"), "{xml}");
        assert_eq!(cell_texts(&doc, 0), [["x"]]);
    }
}

#[test]
fn a_save_keeps_cell_properties_written_without_a_value() {
    for properties in [
        "<w:tcPr><w:vAlign/></w:tcPr>",
        "<w:tcPr><w:gridSpan/></w:tcPr>",
        "<w:tcPr><w:cnfStyle/><w:tcW w:w=\"2000\" w:type=\"dxa\"/><w:vAlign/></w:tcPr>",
    ] {
        let mut doc = doc_with_body(&one_cell_table(properties, "<w:p/>"));
        doc.try_set_cell_text(0, 0, 0, "x").unwrap();
        let doc = reopen(&mut doc);
        let xml = doc.part_xml("word/document.xml").unwrap();
        assert!(xml.contains(&format!("<w:tc>{properties}<w:p>")), "{xml}");
    }
}

#[test]
fn a_save_keeps_empty_row_properties() {
    // Children written without the value they are read by stay as written.
    let valueless = r#"<w:trPr><w:cnfStyle/><w:gridBefore/><w:gridAfter/><w:trHeight w:hRule="exact"/><w:jc/></w:trPr>"#;
    for (properties, saved) in [
        ("<w:trPr/>", "<w:trPr/>"),
        ("<w:trPr></w:trPr>", "<w:trPr/>"),
        (valueless, valueless),
    ] {
        let body = format!(
            r#"<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid><w:tr>{properties}<w:tc><w:p/></w:tc></w:tr></w:tbl><w:p/>"#
        );
        let mut doc = doc_with_body(&body);
        doc.try_set_cell_text(0, 0, 0, "x").unwrap();
        let doc = reopen(&mut doc);
        let xml = doc.part_xml("word/document.xml").unwrap();
        assert!(xml.contains(&format!("<w:tr>{saved}<w:tc>")), "{xml}");
    }
}

#[test]
fn a_row_added_beside_empty_properties_writes_none() {
    // A new row and its cells write only the formatting they carry.
    let mut doc = doc_with_body(
        r#"<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:trPr/><w:tc><w:tcPr/><w:p/></w:tc></w:tr></w:tbl><w:p/>"#,
    );
    doc.table_mut(0).unwrap().add_row().unwrap();
    // The new row has no properties in the model either, so an export does
    // not report row formatting it would drop.
    let odt = doc.to_odt_bytes().unwrap();
    let paths = odt
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.path.as_str())
        .collect::<Vec<_>>();
    assert!(
        !paths.iter().any(|path| path.ends_with("row[1]/trPr")),
        "{paths:?}"
    );
    let doc = reopen(&mut doc);
    let xml = doc.part_xml("word/document.xml").unwrap();
    assert!(
        xml.contains(
            "<w:tr><w:trPr/><w:tc><w:tcPr/><w:p/></w:tc></w:tr><w:tr><w:tc><w:p/></w:tc></w:tr>"
        ),
        "{xml}"
    );
}
