//! Body tables: adding and inserting them, reading and setting cell text,
//! rows and columns, merges and splits, nested tables, widths, borders,
//! shading, styles and header rows, on new documents and the corpus reports.

use oxoffice_docx::{
    Alignment, BorderStyle, CellBorderEdge, CellItemRef, CellTextDirection, CommentDate, Document,
    Length, RowHeight, StyleType, TableBorderEdge, TableCellMargins, TableConditionalFormatting,
    TableFloatX, TableLayout, TableLook, TableTextDistance, TableWidth, TextCommentOptions, VMerge,
    VerticalAlignment,
};

use crate::support::{corpus_doc, doc_with, doc_with_body, reopen, texts};

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
fn table_widths_written_as_a_percent_or_with_a_unit_open_and_read_back() {
    // ISO/IEC 29500 lets w:w be a percent or a universal measure; the
    // spelled form says the type, whatever w:type holds.
    let body = concat!(
        r#"<w:tbl><w:tblPr><w:tblW w:w="33.3%" w:type="pct"/><w:tblInd w:w="0.5in" w:type="dxa"/>"#,
        r#"<w:tblCellSpacing w:w="2pt" w:type="pct"/><w:tblCellMar><w:left w:w="0.25cm" w:type="dxa"/></w:tblCellMar>"#,
        r#"</w:tblPr><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid>"#,
        r#"<w:tr><w:trPr><w:wBefore w:w="0.29%" w:type="pct"/><w:wAfter w:w="-1pc" w:type="dxa"/></w:trPr>"#,
        r#"<w:tc><w:tcPr><w:tcW w:w="50%" w:type="dxa"/></w:tcPr><w:p/></w:tc></w:tr></w:tbl><w:p/>"#,
    );
    let mut doc = doc_with_body(body);
    let table = doc.table(0).expect("the table");
    assert_eq!(table.width_mode(), Some(TableWidth::Percentage(33.3)));
    assert_eq!(table.indent(), Some(Length::twips(720)));
    assert_eq!(
        table.cell_spacing(),
        Some(TableWidth::Fixed(Length::twips(40)))
    );
    let row = table.row(0).expect("the row");
    // Half a fiftieth rounds away from zero, in exact decimal arithmetic.
    assert_eq!(row.width_before(), Some(TableWidth::Percentage(0.3)));
    assert_eq!(
        row.width_after(),
        Some(TableWidth::Fixed(Length::twips(-240)))
    );

    // A rewrite stores them as Word does: fiftieths of a percent and twips.
    doc.table_mut(0)
        .expect("the table")
        .set_alignment(Alignment::Center);
    let doc = reopen(&mut doc);
    let xml = doc.part_xml("word/document.xml").expect("the body");
    for written in [
        r#"<w:tblW w:w="1665" w:type="pct"/>"#,
        r#"<w:tblInd w:w="720" w:type="dxa"/>"#,
        r#"<w:tblCellSpacing w:w="40" w:type="dxa"/>"#,
        r#"<w:left w:w="142" w:type="dxa"/>"#,
        r#"<w:wBefore w:w="15" w:type="pct"/>"#,
        r#"<w:tcW w:w="2500" w:type="pct"/>"#,
    ] {
        assert!(xml.contains(written), "{written}: {xml}");
    }

    // A zero spelled with a unit or a percent reads as zero.
    for (spelled, read) in [
        (
            r#"<w:tblW w:w="0%" w:type="pct"/>"#,
            TableWidth::Percentage(0.0),
        ),
        (
            r#"<w:tblW w:w="0pt" w:type="dxa"/>"#,
            TableWidth::Fixed(Length::twips(0)),
        ),
        (
            r#"<w:tblW w:w="00cm" w:type="dxa"/>"#,
            TableWidth::Fixed(Length::twips(0)),
        ),
        (
            r#"<w:tblW w:w="-0pt" w:type="dxa"/>"#,
            TableWidth::Fixed(Length::twips(0)),
        ),
    ] {
        let zero = doc_with_body(&format!(
            r#"<w:tbl><w:tblPr>{spelled}</w:tblPr><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:tc><w:p/></w:tc></w:tr></w:tbl><w:p/>"#
        ));
        assert_eq!(zero.table(0).unwrap().width_mode(), Some(read), "{spelled}");
    }

    // Inside a block content control the table is read the same way.
    let doc = doc_with_body(&format!(
        "<w:sdt><w:sdtPr><w:tag w:val=\"rows\"/></w:sdtPr><w:sdtContent>{}</w:sdtContent></w:sdt><w:p/>",
        body.trim_end_matches("<w:p/>")
    ));
    assert_eq!(doc.content_controls().len(), 1);
    assert_eq!(
        doc.table(0).expect("the table in the control").width_mode(),
        Some(TableWidth::Percentage(33.3))
    );
}

#[test]
fn a_malformed_table_width_is_refused() {
    for (element, refused) in [
        (r#"<w:tblW w:w="%" w:type="pct"/>"#, "%"),
        (r#"<w:tblW w:w="1e2%" w:type="pct"/>"#, "1e2%"),
        (r#"<w:tblW w:w="5.%" w:type="pct"/>"#, "5.%"),
        (r#"<w:tblW w:w=".5in" w:type="dxa"/>"#, ".5in"),
        (r#"<w:tblW w:w="+5%" w:type="pct"/>"#, "+5%"),
        (
            r#"<w:tblW w:w="999999999999999.999999999999999999cm" w:type="dxa"/>"#,
            "999999999999999.999999999999999999cm",
        ),
        (
            r#"<w:tblW w:w="118153599625325855.369227294247141741in" w:type="dxa"/>"#,
            "118153599625325855.369227294247141741in",
        ),
    ] {
        let body = format!(
            r#"<w:tbl><w:tblPr>{element}</w:tblPr><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:tc><w:p/></w:tc></w:tr></w:tbl><w:p/>"#
        );
        let bytes = Document::new().to_bytes().expect("new document saves");
        let mut package = oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(bytes))
            .expect("package opens");
        let part = package.main_document_part().expect("main part");
        let xml = String::from_utf8(package.get_part(&part).unwrap().to_vec()).unwrap();
        let xml = xml.replacen("<w:body>", &format!("<w:body>{body}"), 1);
        package.set_part(&part, xml.into_bytes());
        let mut out = std::io::Cursor::new(Vec::new());
        package.write_to(&mut out).expect("package writes");
        let error = match Document::from_bytes(&out.into_inner()) {
            Ok(_) => panic!("{element} opened"),
            Err(error) => error.to_string(),
        };
        assert!(
            error.contains(&format!("{refused:?}")),
            "{element}: {error}"
        );
    }
}

#[test]
fn the_unchecked_percentage_setter_keeps_to_what_word_writes() {
    for (percent, stored) in [
        (150.0, 100.0),
        (-5.0, 0.0),
        (f64::INFINITY, 100.0),
        (f64::NEG_INFINITY, 0.0),
    ] {
        let mut doc = Document::new();
        doc.add_table(1, 1).set_width_pct(percent);
        let doc = reopen(&mut doc);
        assert_eq!(
            doc.table(0).unwrap().width_mode(),
            Some(TableWidth::Percentage(stored)),
            "{percent}"
        );
    }
    // NaN names no width: the one already set stays.
    let mut doc = Document::new();
    doc.add_table(1, 1).set_width_pct(40.0);
    doc.table_mut(0).unwrap().set_width_pct(f64::NAN);
    let doc = reopen(&mut doc);
    assert_eq!(
        doc.table(0).unwrap().width_mode(),
        Some(TableWidth::Percentage(40.0))
    );
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

/// A one-cell table whose cell holds these properties and this paragraph.
fn one_cell_table(properties: &str, paragraph: &str) -> String {
    format!(
        r#"<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:tc>{properties}{paragraph}</w:tc></w:tr></w:tbl><w:p/>"#
    )
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
fn an_in_place_row_or_cell_edit_applies_properties_and_declines_content() {
    let mut doc = doc_with_body(
        r#"<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w="2000"/><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:tc><w:tcPr><w:tcW w:w="2000" w:type="dxa"/></w:tcPr><w:p><w:r><w:t>a</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>bare</w:t></w:r></w:p></w:tc></w:tr></w:tbl><w:p/>"#,
    );
    let before = doc.to_bytes().expect("the document saves");
    let declined = doc
        .edit_table_row_properties_in_place(0, 0, |row| {
            row.set_height(Length::pt(20.0));
            row.cell(0).expect("the cell").set_text("changed");
            Ok(())
        })
        .expect("the edit runs");
    assert_eq!(declined, None, "cell text is content, not a row property");
    let declined = doc
        .edit_table_cell_properties_in_place(0, 0, 1, |cell| {
            cell.set_vertical_alignment(VerticalAlignment::Center);
            Ok(())
        })
        .expect("the edit runs");
    assert_eq!(declined, None, "a first w:tcPr moves the cell's content");
    let refused = doc.edit_table_cell_properties_in_place(0, 0, 0, |cell| {
        cell.set_vertical_alignment(VerticalAlignment::Center);
        Err::<(), _>(oxoffice_docx::Error::Other("refused".to_owned()))
    });
    assert!(refused.is_err());
    assert!(
        doc.edit_table_row_properties_in_place(0, 1, |_| Ok(()))
            .is_err()
    );
    assert!(
        doc.edit_table_cell_properties_in_place(0, 0, 2, |_| Ok(()))
            .is_err()
    );
    assert_eq!(doc.to_bytes().expect("the document saves"), before);

    let applied = doc
        .edit_table_row_properties_in_place(0, 0, |row| {
            row.set_height(Length::pt(20.0));
            Ok(())
        })
        .expect("the edit runs");
    assert_eq!(applied, Some(()));
    let applied = doc
        .edit_table_cell_properties_in_place(0, 0, 0, |cell| {
            cell.set_vertical_alignment(VerticalAlignment::Center);
            Ok(())
        })
        .expect("the edit runs");
    assert_eq!(applied, Some(()));
    let doc = reopen(&mut doc);
    let table = doc.table(0).expect("the table");
    assert!(table.row(0).expect("the row").height().is_some());
    assert_eq!(
        table.cell(0, 0).expect("the cell").vertical_alignment(),
        Some(VerticalAlignment::Center)
    );
    assert_eq!(
        table.cell(0, 1).expect("the cell").vertical_alignment(),
        None
    );
}

#[test]
fn an_in_place_row_or_cell_edit_of_a_table_with_controls_is_declined() {
    let cell = r#"<w:tc><w:tcPr><w:tcW w:w="2000" w:type="dxa"/></w:tcPr><w:p><w:r><w:t>a</w:t></w:r></w:p></w:tc>"#;
    let grid = r#"<w:tblPr/><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid>"#;
    // A table with a row control declines both; a row whose cells sit in a cell control keeps the row's own
    // properties in place, as the whole-table route did, finding the control unchanged.
    for (table, row_edit) in [
        (
            format!(
                "<w:tbl>{grid}<w:sdt><w:sdtPr/><w:sdtContent><w:tr>{cell}</w:tr></w:sdtContent></w:sdt></w:tbl><w:p/>"
            ),
            None,
        ),
        (
            format!(
                "<w:tbl>{grid}<w:tr><w:sdt><w:sdtPr/><w:sdtContent>{cell}</w:sdtContent></w:sdt></w:tr></w:tbl><w:p/>"
            ),
            Some(()),
        ),
    ] {
        let mut doc = doc_with_body(&table);
        let row = doc
            .edit_table_row_properties_in_place(0, 0, |row| {
                row.set_height(Length::pt(20.0));
                Ok(())
            })
            .expect("the edit runs");
        let cell = doc
            .edit_table_cell_properties_in_place(0, 0, 0, |cell| {
                cell.set_vertical_alignment(VerticalAlignment::Center);
                Ok(())
            })
            .expect("the edit runs");
        assert_eq!((row, cell), (row_edit, None), "{table}");
    }
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
fn a_row_height_rule_without_a_value_is_formatting_a_row_edit_can_copy_and_set() {
    // The value of `w:trHeight` is optional: a rule alone is row formatting
    // like any other, not opaque XML.
    let body = r#"<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:trPr><w:trHeight w:hRule="exact"/></w:trPr><w:tc><w:p/></w:tc></w:tr></w:tbl><w:p/>"#;
    let mut doc = doc_with_body(body);
    doc.table_mut(0).unwrap().add_row().unwrap();
    let mut doc = reopen(&mut doc);
    let xml = doc.part_xml("word/document.xml").unwrap();
    assert_eq!(
        xml.matches(r#"<w:trPr><w:trHeight w:hRule="exact"/></w:trPr>"#)
            .count(),
        2,
        "{xml}"
    );

    doc.table_mut(0)
        .unwrap()
        .row(0)
        .unwrap()
        .set_height_exact(oxoffice_docx::Length::twips(300));
    let doc = reopen(&mut doc);
    let xml = doc.part_xml("word/document.xml").unwrap();
    assert!(
        xml.contains(r#"<w:tr><w:trPr><w:trHeight w:val="300" w:hRule="exact"/></w:trPr>"#),
        "one height, the one set: {xml}"
    );
    assert_eq!(xml.matches("<w:trHeight").count(), 2, "{xml}");

    // A row height without attributes means the default: a row added beside
    // it is not refused, and a height set replaces it.
    let bare = r#"<w:trPr><w:trHeight/></w:trPr>"#;
    let mut doc =
        doc_with_body(&body.replace(r#"<w:trPr><w:trHeight w:hRule="exact"/></w:trPr>"#, bare));
    doc.table_mut(0).unwrap().add_row().unwrap();
    doc.table_mut(0)
        .unwrap()
        .row(0)
        .unwrap()
        .set_height_exact(oxoffice_docx::Length::twips(300));
    let doc = reopen(&mut doc);
    let xml = doc.part_xml("word/document.xml").unwrap();
    assert!(
        xml.contains(r#"<w:tr><w:trPr><w:trHeight w:val="300" w:hRule="exact"/></w:trPr>"#),
        "{xml}"
    );
    assert_eq!(xml.matches("<w:trHeight").count(), 1, "{xml}");

    // A row height with an attribute the model does not hold stays as written.
    let foreign = r#"<w:trHeight xmlns:x="urn:example:x" w:hRule="exact" x:keep="1"/>"#;
    let mut doc = doc_with_body(&body.replace(r#"<w:trHeight w:hRule="exact"/>"#, foreign));
    doc.try_set_cell_text(0, 0, 0, "x").unwrap();
    let doc = reopen(&mut doc);
    let xml = doc.part_xml("word/document.xml").unwrap();
    assert!(xml.contains(r#"x:keep="1""#), "{xml}");
}

#[test]
fn a_conditional_format_written_with_booleans_only_is_read_and_kept_as_written() {
    // The Transitional boolean form of `w:cnfStyle`, without `w:val`, reads
    // as the value it stands for and is written back as read.
    let row_cnf = r#"<w:cnfStyle w:firstRow="1" w:lastRow="0"/>"#;
    let cell_cnf = r#"<w:cnfStyle w:firstRow="true" w:firstRowFirstColumn="1"/>"#;
    let body = format!(
        r#"<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:trPr>{row_cnf}</w:trPr><w:tc><w:tcPr>{cell_cnf}</w:tcPr><w:p/></w:tc></w:tr></w:tbl><w:p/>"#
    );
    let mut doc = doc_with_body(&body);
    {
        let table = doc.table(0).unwrap();
        let row = table.row(0).unwrap();
        let formatting = row.conditional_formatting().expect("row formatting");
        assert!(formatting.first_row && !formatting.last_row);
        let cell = row
            .cell(0)
            .unwrap()
            .conditional_formatting()
            .expect("cell formatting");
        assert!(cell.first_row && cell.first_row_first_column && !cell.last_column);
    }
    // A row added beside it and a column inserted beside it copy it.
    doc.table_mut(0).unwrap().add_row().unwrap();
    doc.table_mut(0)
        .unwrap()
        .insert_column(0, false, None)
        .unwrap();
    let doc = reopen(&mut doc);
    let xml = doc.part_xml("word/document.xml").unwrap();
    assert_eq!(
        xml.matches(&format!("<w:trPr>{row_cnf}</w:trPr>")).count(),
        2,
        "{xml}"
    );
    assert_eq!(xml.matches(cell_cnf).count(), 4, "{xml}");
    assert!(!xml.contains("w:val=\"1"), "{xml}");
}

#[test]
fn table_and_cell_edges_set_to_nil_or_none_draw_no_border() {
    let table = |borders: &str, cell_borders: &str| {
        format!(
            r#"<w:tbl><w:tblPr><w:tblBorders>{borders}</w:tblBorders></w:tblPr><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:tc><w:tcPr><w:tcBorders>{cell_borders}</w:tcBorders></w:tcPr><w:p><w:r><w:t>cell</w:t></w:r></w:p></w:tc></w:tr></w:tbl><w:p/>"#
        )
    };
    let undrawn = table(
        r#"<w:top w:val="nil"/><w:left w:val="none" w:sz="0" w:space="0" w:color="auto"/><w:insideH w:val="nil"/>"#,
        r#"<w:top w:val="nil"/><w:bottom w:val="none"/>"#,
    );
    let drawn = table(
        r#"<w:top w:val="nil"/><w:bottom w:val="single" w:sz="4" w:space="0" w:color="000000"/>"#,
        r#"<w:top w:val="nil"/><w:right w:val="single" w:sz="4" w:space="0" w:color="000000"/>"#,
    );

    let doc = doc_with_body(&undrawn);
    let table_ref = doc.table(0).unwrap();
    assert!(!table_ref.has_borders(), "nil and none edges only");
    assert!(!table_ref.cell(0, 0).unwrap().has_borders());
    assert!(
        !doc.to_html_fragment().contains("border:"),
        "{}",
        doc.to_html_fragment()
    );

    let doc = doc_with_body(&drawn);
    let table_ref = doc.table(0).unwrap();
    assert!(table_ref.has_borders(), "one drawn edge among nil ones");
    assert!(table_ref.cell(0, 0).unwrap().has_borders());
    assert_eq!(
        doc.to_html_fragment()
            .matches("border:1px solid #000")
            .count(),
        2,
        "{}",
        doc.to_html_fragment()
    );
}

#[test]
fn rtf_import_reads_a_table_as_word_writes_its_rows() {
    // Word restates each row's definition after its last cell, just before
    // `\row`; the second `\trowd` belongs to the same row.
    let rtf = br"{\rtf1\ansi\deff0{\fonttbl{\f0 Calibri;}}
\trowd \irow0\trgaph108\cellx2000\cellx4000\pard\plain \intbl a\cell \pard\plain \intbl b\cell
\pard\plain \intbl {\trowd \irow0\trgaph108\cellx2000\cellx4000\row }
\trowd \irow1\lastrow \trgaph108\cellx2000\cellx4000\pard\plain \intbl c\cell \pard\plain \intbl d\cell
\pard\plain \intbl {\trowd \irow1\lastrow \trgaph108\cellx2000\cellx4000\row }
\pard\plain after\par}";
    let doc = Document::from_rtf_bytes(rtf).unwrap().document;
    let table = doc.table(0).unwrap();
    let cells: Vec<Vec<String>> = (0..2)
        .map(|row| {
            (0..2)
                .map(|column| table.cell(row, column).unwrap().text())
                .collect()
        })
        .collect();
    assert_eq!(cells, [["a", "b"], ["c", "d"]]);
    assert_eq!(texts(&doc).last().map(String::as_str), Some("after"));
}

#[test]
fn exports_report_no_formatting_for_empty_property_elements() {
    // LibreOffice writes an empty `w:trPr` in every row, and empty `w:pPr`,
    // `w:rPr` and `w:tcPr` elements: they carry no formatting to drop.
    let doc = doc_with_body(concat!(
        r#"<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid>"#,
        r#"<w:tr><w:trPr/><w:tc><w:tcPr/><w:p><w:pPr><w:rPr/></w:pPr><w:r><w:rPr/><w:t>a</w:t></w:r></w:p></w:tc></w:tr>"#,
        r#"<w:tr><w:trPr></w:trPr><w:tc><w:tcPr></w:tcPr><w:p><w:pPr><w:rPr></w:rPr></w:pPr><w:r><w:t>b</w:t></w:r></w:p></w:tc></w:tr>"#,
        // A height without attributes means the default height.
        r#"<w:tr><w:trPr><w:trHeight/></w:trPr><w:tc><w:p><w:r><w:t>c</w:t></w:r></w:p></w:tc></w:tr>"#,
        r#"</w:tbl><w:p><w:pPr><w:rPr/></w:pPr><w:r><w:t>after</w:t></w:r></w:p>"#,
    ));
    let doc = reopen(&mut { doc });
    let odt = doc.to_odt_bytes().unwrap();
    let rtf = doc.to_rtf_bytes().unwrap();
    let epub = doc.to_epub_bytes().unwrap();
    let mhtml = doc.to_mhtml_bytes().unwrap();
    let reported: Vec<String> = odt
        .diagnostics
        .iter()
        .map(|d| format!("odt {} {}", d.path, d.message))
        .chain(
            rtf.diagnostics
                .iter()
                .map(|d| format!("rtf {:?} {}", d.destination, d.message)),
        )
        .chain(
            epub.diagnostics
                .iter()
                .map(|d| format!("epub {} {}", d.path, d.message)),
        )
        .chain(
            mhtml
                .diagnostics
                .iter()
                .map(|d| format!("mhtml {} {}", d.location, d.message)),
        )
        .filter(|line| {
            ["trPr", "tcPr", "rPr", "pPr", "row properties", "mark"]
                .iter()
                .any(|word| line.contains(word))
        })
        // The new document's defaults name another complex-script font,
        // which ODT reports on every run: not what this test is about.
        .filter(|line| !line.contains("unsupported run properties"))
        .collect();
    assert!(reported.is_empty(), "{reported:#?}");

    // A row or a paragraph mark that does carry formatting is still reported.
    let doc = doc_with_body(concat!(
        r#"<w:tbl><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid>"#,
        r#"<w:tr><w:trPr><w:cantSplit/></w:trPr><w:tc><w:p><w:r><w:t>a</w:t></w:r></w:p></w:tc></w:tr>"#,
        r#"</w:tbl><w:p><w:pPr><w:rPr><w:b/></w:rPr></w:pPr><w:r><w:t>after</w:t></w:r></w:p>"#,
    ));
    let odt = doc.to_odt_bytes().unwrap();
    assert!(
        odt.diagnostics
            .iter()
            .any(|d| d.path == "body[0]/row[0]/trPr"),
        "{:?}",
        odt.diagnostics
    );
    // RTF writes the mark's formatting as the paragraph's.
    let rtf = doc.to_rtf_bytes().unwrap();
    assert!(
        !rtf.diagnostics.iter().any(|d| d
            .destination
            .as_deref()
            .is_some_and(|path| path.contains("/ppr/rPr"))),
        "{:?}",
        rtf.diagnostics
    );
    let text = String::from_utf8(rtf.bytes).unwrap();
    let after = text.split(r"\pard").find(|p| p.contains("after")).unwrap();
    assert!(after.contains(r"\b{\plain"), "{after}");
    let epub = doc.to_epub_bytes().unwrap();
    assert!(
        epub.diagnostics
            .iter()
            .any(|d| d.path.ends_with("mark-properties")),
        "{:?}",
        epub.diagnostics
    );
    assert!(
        odt.diagnostics.iter().any(|d| d.path == "body[1]/pPr"),
        "{:?}",
        odt.diagnostics
    );

    // An element that holds only what the model does not type is not empty.
    let doc = doc_with_body(concat!(
        r#"<w:tbl><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid>"#,
        r#"<w:tr><w:trPr><w:ins w:id="1" w:author="a" w:date="2026-01-01T00:00:00Z"/></w:trPr>"#,
        r#"<w:tc><w:p><w:r><w:t>a</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#,
        r#"<w:p><w:pPr><w:rPr><w:oxofficeUnknown/></w:rPr></w:pPr><w:r><w:t>after</w:t></w:r></w:p>"#,
    ));
    let odt = doc.to_odt_bytes().unwrap();
    assert!(
        odt.diagnostics
            .iter()
            .any(|d| d.path == "body[0]/row[0]/trPr"),
        "{:?}",
        odt.diagnostics
    );
    // What RTF cannot write of the mark is reported where it is.
    let rtf = doc.to_rtf_bytes().unwrap();
    assert!(
        rtf.diagnostics.iter().any(|d| d
            .destination
            .as_deref()
            .is_some_and(|path| path.contains("/ppr/rPr"))),
        "{:?}",
        rtf.diagnostics
    );
}

#[test]
fn exports_report_no_list_level_formatting_for_empty_property_elements() {
    // A list level that holds empty `w:pPr` and `w:rPr` elements.
    let mut doc = Document::new();
    doc.add_numbered_list_item("item", 0);
    let bytes = doc.to_bytes().unwrap();
    let mut package = oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(bytes)).unwrap();
    let xml = String::from_utf8(package.get_part("/word/numbering.xml").unwrap().to_vec()).unwrap();
    let start = xml.find("<w:lvl w:ilvl=\"0\"").unwrap();
    let end = start + xml[start..].find("</w:lvl>").unwrap();
    let level = &xml[start..end];
    let edited = match (level.find("<w:pPr>"), level.find("</w:pPr>")) {
        (Some(open), Some(close)) => format!(
            "{}<w:pPr/><w:rPr/>{}",
            &level[..open],
            &level[close + "</w:pPr>".len()..]
        ),
        _ => format!("{level}<w:pPr/><w:rPr/>"),
    };
    let xml = format!("{}{edited}{}", &xml[..start], &xml[end..]);
    package.set_part("/word/numbering.xml", xml.into_bytes());
    let mut out = std::io::Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    let doc = Document::from_bytes(&out.into_inner()).unwrap();

    let odt = doc.to_odt_bytes().unwrap();
    let epub = doc.to_epub_bytes().unwrap();
    let reported: Vec<&str> = odt
        .diagnostics
        .iter()
        .map(|d| d.message.as_str())
        .chain(epub.diagnostics.iter().map(|d| d.message.as_str()))
        .filter(|message| {
            message.starts_with("list level paragraph formatting")
                || message.starts_with("list marker run formatting")
                || message.starts_with("list-level paragraph styling")
                || message.starts_with("list marker run styling")
        })
        .collect();
    assert!(reported.is_empty(), "{reported:?}");
}

#[test]
fn table_twips_measures_written_with_a_unit_open_and_a_percent_cell_margin_is_ignored() {
    // ST_TwipsMeasure and ST_SignedTwipsMeasure take a universal measure in
    // Transitional; Word ignores a cell margin written as a percent.
    let body = concat!(
        r#"<w:tbl><w:tblPr><w:tblpPr w:leftFromText="0.1in" w:rightFromText="9pt" w:topFromText="1cm" "#,
        r#"w:bottomFromText="0" w:tblpX="-0.5in" w:tblpY="2pc"/>"#,
        r#"<w:tblCellMar><w:left w:w="5%" w:type="pct"/><w:right w:w="0.1in" w:type="dxa"/></w:tblCellMar>"#,
        r#"</w:tblPr><w:tblGrid><w:gridCol w:w="2in"/><w:gridCol w:w="1.5cm"/></w:tblGrid>"#,
        r#"<w:tr><w:trPr><w:trHeight w:val="0.5in" w:hRule="exact"/></w:trPr>"#,
        r#"<w:tc><w:p/></w:tc><w:tc><w:p/></w:tc></w:tr></w:tbl><w:p/>"#,
    );
    let doc = doc_with_body(body);
    let table = doc.table(0).expect("the table");
    assert_eq!(
        table.grid_widths(),
        [Length::twips(2880), Length::twips(850)]
    );
    assert_eq!(
        table.row(0).unwrap().height(),
        Some(RowHeight::Exact(Length::twips(720)))
    );
    let float = table.float_position().expect("the float position");
    assert_eq!(
        float.distance_from_text,
        TableTextDistance {
            top: Length::twips(567),
            right: Length::twips(180),
            bottom: Length::twips(0),
            left: Length::twips(144),
        }
    );
    assert_eq!(float.horizontal, TableFloatX::Offset(Length::twips(-720)));
    let margins = table.cell_margins().expect("the margins");
    assert_eq!(margins.left, None, "the percent margin reads as none");
    assert_eq!(margins.right, Some(Length::twips(144)));

    // Inside a block content control the table is read the same way.
    let doc = doc_with_body(&format!(
        "<w:sdt><w:sdtContent>{}</w:sdtContent></w:sdt><w:p/>",
        body.trim_end_matches("<w:p/>")
    ));
    assert_eq!(
        doc.table(0)
            .expect("the table in the control")
            .grid_widths(),
        [Length::twips(2880), Length::twips(850)]
    );

    // Table XML replaced through the checked route takes the same forms.
    let mut doc = doc_with_body(
        r#"<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:tc><w:p/></w:tc></w:tr></w:tbl><w:p/>"#,
    );
    let body_story = doc
        .stories()
        .unwrap()
        .into_iter()
        .find(|story| story.kind() == oxoffice_docx::StoryKind::Body)
        .unwrap();
    let location = doc.story_items(&body_story).unwrap()[0].location().clone();
    doc.replace_table_xml(
        &location,
        concat!(
            r#"<w:tbl xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:tblPr/>"#,
            r#"<w:tblGrid><w:gridCol w:w="2in"/></w:tblGrid><w:tr><w:trPr><w:trHeight w:val="0.5in"/></w:trPr>"#,
            r#"<w:tc><w:p/></w:tc></w:tr></w:tbl>"#
        ),
    )
    .expect("the checked route reads a twips measure with a unit");
    assert_eq!(doc.table(0).unwrap().grid_widths(), [Length::twips(2880)]);

    // A twips measure takes no percent, and a malformed unit is refused.
    for (element, refused) in [
        (r#"<w:gridCol w:w="5%"/>"#, "5%"),
        (r#"<w:gridCol w:w="2.in"/>"#, "2.in"),
    ] {
        let body = format!(
            r#"<w:tbl><w:tblPr/><w:tblGrid>{element}</w:tblGrid><w:tr><w:tc><w:p/></w:tc></w:tr></w:tbl><w:p/>"#
        );
        let bytes = Document::new().to_bytes().expect("new document saves");
        let mut package = oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(bytes))
            .expect("package opens");
        let part = package.main_document_part().expect("main part");
        let xml = String::from_utf8(package.get_part(&part).unwrap().to_vec()).unwrap();
        let xml = xml.replacen("<w:body>", &format!("<w:body>{body}"), 1);
        package.set_part(&part, xml.into_bytes());
        let mut out = std::io::Cursor::new(Vec::new());
        package.write_to(&mut out).unwrap();
        let error = Document::from_bytes(&out.into_inner())
            .err()
            .expect("refused");
        assert!(error.to_string().contains(refused), "{refused}: {error}");
    }
}
