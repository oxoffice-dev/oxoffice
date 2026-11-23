//! Paragraphs and styles: the order of body content (insert, move, copy,
//! remove), paragraph formatting, headings, paragraph and character styles,
//! document defaults and the properties a paragraph or run ends up with.

use oxoffice_docx::{
    Alignment, BodyItemRef, BorderStyle, CT_PPr, ContentLocation, Document, Length,
    ParagraphBorderEdge, StoryKind, StyleBuilder, StyleType, TabAlignment, TabLeader, Twips,
};
use oxoffice_docx_oxml::shared::ST_Jc;
use oxoffice_opc::OpcPackage;
use std::io::Cursor;

use crate::support::{corpus_doc, doc_with, doc_with_body, reopen, texts};

/// The checked body location of the paragraph `index` of `doc.paragraphs()`.
fn paragraph_location(doc: &Document, index: usize) -> ContentLocation {
    doc.paragraph_story_location(index)
        .expect("paragraph locations resolve")
        .unwrap_or_else(|| panic!("paragraph {index} is a direct body paragraph"))
}

/// A length in whole twips, the unit Word stores paragraph measures in.
fn twips(length: Option<Length>) -> Option<i32> {
    length.map(Length::to_twips)
}

/// The kinds of the direct body items, `p` for a paragraph and `t` for a table.
fn body_kinds(doc: &Document) -> String {
    doc.body_items()
        .map(|item| match item {
            BodyItemRef::Paragraph(_) => 'p',
            BodyItemRef::Table(_) => 't',
            _ => '?',
        })
        .collect()
}

// ---- Order of body content ----

#[test]
fn inserted_paragraphs_land_where_asked_and_survive_a_save() {
    let mut doc = doc_with(&["middle"]);
    doc.insert_paragraph(0, "first");
    let end = doc.content_count();
    doc.insert_paragraph(end, "last");
    doc.insert_paragraph(1, "second");
    // An empty paragraph keeps its style and spacing, as an empty heading does in Word.
    doc.add_paragraph("")
        .style("Heading1")
        .space_after(Length::pt(5.0));
    assert!(doc.try_insert_paragraph(99, "nowhere").is_err());

    let reopened = reopen(&mut doc);
    assert_eq!(texts(&reopened), ["first", "second", "middle", "last", ""]);
    let empty = reopened.paragraph(4).unwrap();
    assert_eq!(empty.run_count(), 0);
    assert_eq!(empty.style_id(), Some("Heading1"));
    assert_eq!(twips(empty.space_after()), Some(100));
}

#[test]
fn body_indexes_count_tables_and_paragraph_indexes_skip_them() {
    let mut doc = doc_with(&["before", "after"]);
    doc.insert_table(1, 1, 1)
        .cell(0, 0)
        .unwrap()
        .set_text("cell");
    // Body index 1 is the table: the new paragraph goes in front of it.
    doc.insert_paragraph(1, "above the table");
    assert_eq!(body_kinds(&doc), "pptp");
    assert_eq!(doc.paragraph_index_of_content(3), Some(2));
    assert_eq!(doc.paragraph_index_of_content(2), None);
    assert_eq!(doc.content_index_of_paragraph(2), Some(3));

    let reopened = reopen(&mut doc);
    assert_eq!(body_kinds(&reopened), "pptp");
    assert_eq!(texts(&reopened), ["before", "above the table", "after"]);
}

#[test]
fn content_is_removed_by_index_or_by_half_open_range() {
    let mut doc = doc_with(&["zero", "one", "two", "three", "four"]);
    assert!(doc.remove_content(1));
    assert!(!doc.remove_content(10));
    assert_eq!(texts(&doc), ["zero", "two", "three", "four"]);

    assert_eq!(doc.remove_content_range(1, 1).unwrap(), 0);
    assert_eq!(doc.remove_content_range(1, 3).unwrap(), 2);
    assert_eq!(texts(&doc), ["zero", "four"]);

    // A reversed or out-of-range range is refused and changes nothing.
    let before = doc.to_bytes().unwrap();
    assert!(doc.remove_content_range(2, 1).is_err());
    assert!(doc.remove_content_range(0, 3).is_err());
    assert_eq!(doc.to_bytes().unwrap(), before);
    assert_eq!(texts(&reopen(&mut doc)), ["zero", "four"]);
}

#[test]
fn a_paragraph_moves_past_a_table_but_not_into_another_story() {
    let mut doc = doc_with(&["before", "after"]);
    doc.insert_table(1, 1, 1)
        .cell(0, 0)
        .unwrap()
        .set_text("cell");
    let after = paragraph_location(&doc, 1);
    let front = paragraph_location(&doc, 0);
    doc.move_content(&after, &front).unwrap();
    assert_eq!(body_kinds(&doc), "ppt");
    assert_eq!(texts(&doc), ["after", "before"]);

    doc.set_header("Header text");
    let header = doc
        .stories()
        .unwrap()
        .into_iter()
        .find(|story| story.kind() == StoryKind::Header)
        .expect("a header story");
    let before = doc.to_bytes().unwrap();
    let error = doc
        .move_content(&paragraph_location(&doc, 0), &ContentLocation::end(header))
        .unwrap_err();
    assert!(error.to_string().contains("within one"), "{error}");
    assert_eq!(doc.to_bytes().unwrap(), before);

    let reopened = reopen(&mut doc);
    assert_eq!(body_kinds(&reopened), "ppt");
    assert_eq!(texts(&reopened), ["after", "before"]);
}

// ---- Paragraph formatting ----

#[test]
fn paragraph_layout_settings_survive_a_save() {
    let mut doc = Document::new();
    doc.add_paragraph("measured")
        .alignment(Alignment::Justify)
        .indent_left(Length::inches(1.0))
        .indent_right(Length::inches(0.5))
        .first_line_indent(Length::pt(18.0))
        .space_before(Length::pt(6.0))
        .space_after(Length::pt(12.0))
        .line_spacing_multiple(1.5)
        .outline_level(2);
    // A negative first-line indent is a hanging indent.
    let mut hanging = doc.add_paragraph("hanging");
    hanging.set_signed_first_line_indent_value(Some(Length::pt(-18.0)));
    // Word knows outline levels 0 to 9 only.
    assert!(!hanging.set_outline_level_value(Some(10)));

    let reopened = reopen(&mut doc);
    let measured = reopened.paragraph(0).unwrap();
    assert_eq!(measured.alignment(), Some(Alignment::Justify));
    assert_eq!(twips(measured.indent_left()), Some(1440));
    assert_eq!(twips(measured.indent_right()), Some(720));
    assert_eq!(twips(measured.first_line_indent()), Some(360));
    assert_eq!(twips(measured.space_before()), Some(120));
    assert_eq!(twips(measured.space_after()), Some(240));
    assert_eq!(measured.line_spacing_multiple(), Some(1.5));
    assert_eq!(measured.outline_level(), Some(2));

    let hanging = reopened.paragraph(1).unwrap();
    assert_eq!(twips(hanging.first_line_indent()), Some(-360));
    assert_eq!(twips(hanging.hanging_indent()), Some(360));
    assert_eq!(hanging.outline_level(), None);
}

#[test]
fn an_empty_paragraph_border_container_read_from_a_file_survives_an_edit() {
    // The paragraph keeps the container as read, beside a property set by
    // the API, as the table and cell border containers do.
    for written in ["<w:pBdr></w:pBdr>", "<w:pBdr/>"] {
        let mut doc = doc_with_body(&format!(
            r#"<w:p><w:pPr>{written}</w:pPr><w:r><w:t>Text</w:t></w:r></w:p>"#
        ));
        doc.paragraph_mut(0)
            .unwrap()
            .set_alignment(Alignment::Center);
        doc.replace_text("Text", "Edited");
        let doc = reopen(&mut doc);
        let xml = doc.part_xml("word/document.xml").unwrap();
        let properties = &xml[xml.find("<w:pPr>").unwrap()..xml.find("</w:pPr>").unwrap()];
        assert!(properties.contains("<w:pBdr"), "{written}: {xml}");
        assert!(
            properties.contains(r#"<w:jc w:val="center"/>"#),
            "{written}: {xml}"
        );
    }
}

#[test]
fn exact_and_multiple_line_spacing_replace_each_other() {
    let mut doc = Document::new();
    doc.add_paragraph("lines")
        .line_spacing_multiple(2.0)
        .line_spacing(14.0);
    let mut reopened = reopen(&mut doc);
    let paragraph = reopened.paragraph(0).unwrap();
    assert_eq!(paragraph.line_spacing().map(Length::to_pt), Some(14.0));
    assert_eq!(paragraph.line_spacing_multiple(), None);

    reopened
        .paragraph_mut(0)
        .unwrap()
        .set_line_spacing_multiple(1.15);
    let mut again = reopen(&mut reopened);
    let paragraph = again.paragraph(0).unwrap();
    assert_eq!(paragraph.line_spacing(), None);
    assert_eq!(paragraph.line_spacing_multiple(), Some(276.0 / 240.0));

    again.paragraph_mut(0).unwrap().clear_line_spacing();
    let cleared = reopen(&mut again);
    assert_eq!(cleared.paragraph(0).unwrap().line_spacing_multiple(), None);
}

#[test]
fn pagination_flags_keep_an_explicit_false() {
    let mut doc = Document::new();
    doc.add_paragraph("kept")
        .keep_with_next(true)
        .keep_together(true)
        .page_break_before(true)
        .widow_control(false);
    doc.add_paragraph("plain");

    let reopened = reopen(&mut doc);
    let kept = reopened.paragraph(0).unwrap();
    assert_eq!(kept.keep_with_next_value(), Some(true));
    assert_eq!(kept.keep_together_value(), Some(true));
    assert_eq!(kept.page_break_before_value(), Some(true));
    assert!(kept.is_page_break_before());
    // An explicit false overrides a style that turns widow control on.
    assert_eq!(kept.widow_control_value(), Some(false));

    let plain = reopened.paragraph(1).unwrap();
    assert_eq!(plain.keep_with_next_value(), None);
    assert_eq!(plain.widow_control_value(), None);
    assert!(!plain.is_page_break_before());
}

#[test]
fn setting_a_first_line_indent_replaces_a_hanging_indent() {
    let mut doc = Document::new();
    doc.add_paragraph("indented")
        .hanging_indent(Length::pt(9.0))
        .first_line_indent(Length::pt(18.0));
    let reopened = reopen(&mut doc);
    let paragraph = reopened.paragraph(0).unwrap();
    assert_eq!(twips(paragraph.first_line_indent()), Some(360));
    assert_eq!(
        twips(paragraph.hanging_indent()),
        None,
        "the first-line indent replaces the hanging indent"
    );
    let xml = reopened.part_xml("word/document.xml").unwrap();
    assert!(
        xml.contains(r#"w:firstLine="360""#) && !xml.contains("w:hanging="),
        "{xml}"
    );
}

#[test]
fn a_negative_first_line_indent_is_written_as_a_hanging_indent() {
    let mut doc = Document::new();
    doc.add_paragraph("by the builder")
        .first_line_indent(Length::pt(-9.0));
    doc.add_paragraph("by the setter");
    doc.paragraph_mut(1)
        .unwrap()
        .set_first_line_indent(Length::pt(-9.0));
    let reopened = reopen(&mut doc);
    for index in 0..2 {
        let paragraph = reopened.paragraph(index).unwrap();
        assert_eq!(twips(paragraph.first_line_indent()), Some(-180), "{index}");
        assert_eq!(twips(paragraph.hanging_indent()), Some(180), "{index}");
    }
    // The schema allows no negative w:firstLine: Word means a hanging indent.
    let xml = reopened.part_xml("word/document.xml").unwrap();
    assert_eq!(xml.matches(r#"w:hanging="180""#).count(), 2, "{xml}");
    assert!(!xml.contains("w:firstLine="), "{xml}");

    // Nor a negative w:hanging: it is a first-line indent.
    let mut doc = Document::new();
    doc.add_paragraph("outdented")
        .hanging_indent(Length::pt(-9.0));
    let reopened = reopen(&mut doc);
    assert_eq!(
        twips(reopened.paragraph(0).unwrap().first_line_indent()),
        Some(180)
    );
    let xml = reopened.part_xml("word/document.xml").unwrap();
    assert!(
        xml.contains(r#"w:firstLine="180""#) && !xml.contains("w:hanging="),
        "{xml}"
    );
}

#[test]
fn a_paragraph_holding_both_indents_reads_and_exports_its_hanging_indent() {
    // Word and layout ignore w:firstLine when w:hanging is there.
    let doc = crate::support::doc_with_body(concat!(
        r#"<w:p><w:pPr><w:ind w:left="720" w:firstLine="360" w:hanging="180"/></w:pPr>"#,
        r#"<w:r><w:t>both</w:t></w:r></w:p>"#
    ));
    assert_eq!(
        twips(doc.paragraph(0).unwrap().first_line_indent()),
        Some(-180)
    );

    let rtf = String::from_utf8(doc.to_rtf_bytes().unwrap().bytes).unwrap();
    assert!(
        rtf.contains(r"\fi-180") && !rtf.contains(r"\fi360"),
        "{rtf}"
    );

    let odt = doc.to_odt_bytes().unwrap();
    let reread = Document::from_odt_bytes(&odt.bytes).unwrap().document;
    assert_eq!(
        twips(reread.paragraph(0).unwrap().first_line_indent()),
        Some(-180),
        "ODT carries the hanging indent"
    );
    let html = doc.to_html_fragment();
    assert!(html.contains("text-indent:-9.0pt"), "{html}");
    let epub = doc.to_epub_bytes().unwrap();
    let chapters: String = zip_names(&epub.bytes)
        .iter()
        .filter(|name| name.ends_with(".xhtml") && !name.ends_with("nav.xhtml"))
        .filter_map(|name| zip_entry(&epub.bytes, name))
        .map(|bytes| String::from_utf8(bytes).unwrap())
        .collect();
    assert!(chapters.contains("text-indent:-9.0pt"), "{chapters}");

    // Layout too, even for a hanging indent of 0.
    let doc = crate::support::doc_with_body(concat!(
        r#"<w:p><w:pPr><w:ind w:left="720"/></w:pPr><w:r><w:t>plain</w:t></w:r></w:p>"#,
        r#"<w:p><w:pPr><w:ind w:left="720" w:firstLine="360" w:hanging="180"/></w:pPr>"#,
        r#"<w:r><w:t>hanging</w:t></w:r></w:p>"#,
        r#"<w:p><w:pPr><w:ind w:left="720" w:firstLine="360" w:hanging="0"/></w:pPr>"#,
        r#"<w:r><w:t>zero</w:t></w:r></w:p>"#
    ));
    let result = doc.layout().unwrap();
    let mut starts = std::collections::HashMap::new();
    for page in &result.layout.pages {
        oxoffice_layout::walk(&page.elements, &mut |element, _| {
            if let oxoffice_layout::PositionedElement::Text(run) = element {
                starts.entry(run.text.clone()).or_insert(run.origin.x);
            }
        });
    }
    let plain = starts["plain"];
    assert!(
        (starts["hanging"] - (plain - 9.0)).abs() < 0.01,
        "{starts:?}"
    );
    assert!((starts["zero"] - plain).abs() < 0.01, "{starts:?}");
}

#[test]
fn borders_and_shading_survive_a_save_and_can_be_removed() {
    let mut doc = Document::new();
    doc.add_paragraph("boxed")
        .border_all(BorderStyle::Single, 4, "000000")
        .border_bottom_with_space(BorderStyle::Double, 12, 4, "FF0000")
        .shading("FFFF00");

    let mut reopened = reopen(&mut doc);
    let boxed = reopened.paragraph(0).unwrap();
    assert!(boxed.has_borders());
    assert_eq!(boxed.border_count(), 4);
    let top = boxed.border(ParagraphBorderEdge::Top).unwrap();
    assert_eq!(
        (top.style(), top.size_eighths_pt(), top.color()),
        ("single", Some(4), Some("000000"))
    );
    let bottom = boxed.bottom_border().unwrap();
    assert_eq!(
        (
            bottom.style(),
            bottom.size_eighths_pt(),
            bottom.space_points(),
            bottom.color()
        ),
        ("double", Some(12), Some(4), Some("FF0000"))
    );
    assert!(boxed.border(ParagraphBorderEdge::Between).is_none());
    assert_eq!(boxed.shading_fill(), Some("FFFF00"));

    let mut paragraph = reopened.paragraph_mut(0).unwrap();
    paragraph.clear_borders();
    paragraph.set_shading_value(None);
    let cleared = reopen(&mut reopened);
    let plain = cleared.paragraph(0).unwrap();
    assert!(!plain.has_borders());
    assert_eq!(plain.shading_fill(), None);
}

#[test]
fn tab_stops_are_kept_in_position_order_with_their_leaders() {
    let mut doc = Document::new();
    let mut paragraph = doc.add_paragraph("Name\tScore\tPage");
    paragraph
        .try_add_tab_stop(
            TabAlignment::Right,
            Length::inches(6.0),
            Some(TabLeader::Dot),
        )
        .unwrap();
    paragraph
        .try_add_tab_stop(TabAlignment::Center, Length::inches(3.0), None)
        .unwrap();
    paragraph
        .try_add_tab_stop(TabAlignment::Decimal, Length::inches(4.5), None)
        .unwrap();
    assert!(paragraph.remove_tab_stop(1));

    let mut reopened = reopen(&mut doc);
    let paragraph = reopened.paragraph(0).unwrap();
    let tabs: Vec<_> = (0..paragraph.tab_stop_count())
        .map(|index| {
            let tab = paragraph.tab_stop(index).unwrap();
            (tab.alignment(), tab.position().to_twips(), tab.leader())
        })
        .collect();
    assert_eq!(
        tabs,
        [
            (Some(TabAlignment::Center), 4320, None),
            (Some(TabAlignment::Right), 8640, Some(TabLeader::Dot)),
        ]
    );

    reopened.paragraph_mut(0).unwrap().clear_tab_stops();
    assert_eq!(
        reopen(&mut reopened).paragraph(0).unwrap().tab_stop_count(),
        0
    );
}

#[test]
fn right_to_left_and_east_asian_paragraphs_survive_a_save() {
    let mut doc = Document::new();
    doc.add_paragraph("مرحبا بالعالم")
        .right_to_left(true)
        .alignment(Alignment::Right);
    let mut japanese = doc.add_paragraph("こんにちは、世界。");
    japanese.set_kinsoku_value(Some(true));
    japanese.set_word_wrap_value(Some(false));

    let reopened = reopen(&mut doc);
    let arabic = reopened.paragraph(0).unwrap();
    assert_eq!(arabic.text(), "مرحبا بالعالم");
    assert_eq!(arabic.right_to_left_value(), Some(true));
    assert_eq!(arabic.alignment(), Some(Alignment::Right));
    let japanese = reopened.paragraph(1).unwrap();
    assert_eq!(japanese.text(), "こんにちは、世界。");
    assert_eq!(japanese.kinsoku_value(), Some(true));
    assert_eq!(japanese.word_wrap_value(), Some(false));
    assert_eq!(japanese.right_to_left_value(), None);
}

// ---- Headings ----

#[test]
fn headings_and_the_outline_follow_the_heading_styles() {
    let mut doc = Document::new();
    doc.add_paragraph("Report").style("Title");
    doc.add_paragraph("Scope").style("Heading1");
    doc.add_paragraph("Detail").style("Heading3");
    doc.add_paragraph("Method").style("Heading2");
    doc.add_paragraph("Body text");
    doc.add_paragraph("Results").style("Heading1");

    let reopened = reopen(&mut doc);
    // The title is not a heading.
    assert_eq!(
        reopened.headings(),
        [
            (1, "Scope".to_owned()),
            (3, "Detail".to_owned()),
            (2, "Method".to_owned()),
            (1, "Results".to_owned()),
        ]
    );
    let outline = reopened.document_outline();
    assert_eq!(outline.len(), 2);
    assert_eq!(outline[0].text, "Scope");
    let children: Vec<_> = outline[0]
        .children
        .iter()
        .map(|node| (node.level, node.text.as_str()))
        .collect();
    assert_eq!(children, [(3, "Detail"), (2, "Method")]);
    assert!(outline[1].children.is_empty());
}

/// The bytes of one entry of a ZIP archive, by name, read through its
/// central directory: stored or deflated.
fn zip_entry(archive: &[u8], wanted: &str) -> Option<Vec<u8>> {
    let u16_at = |at: usize| u16::from_le_bytes([archive[at], archive[at + 1]]) as usize;
    let u32_at = |at: usize| u32::from_le_bytes(archive[at..at + 4].try_into().unwrap()) as usize;
    let end = (0..archive.len().saturating_sub(21))
        .rev()
        .find(|&at| archive[at..at + 4] == [0x50, 0x4b, 0x05, 0x06])?;
    let mut at = u32_at(end + 16);
    for _ in 0..u16_at(end + 10) {
        let (method, size) = (u16_at(at + 10), u32_at(at + 20));
        let (name_len, extra_len, comment_len) =
            (u16_at(at + 28), u16_at(at + 30), u16_at(at + 32));
        let name = std::str::from_utf8(&archive[at + 46..at + 46 + name_len]).ok()?;
        if name == wanted {
            let local = u32_at(at + 42);
            let data = local + 30 + u16_at(local + 26) + u16_at(local + 28);
            let raw = &archive[data..data + size];
            return match method {
                0 => Some(raw.to_vec()),
                8 => miniz_oxide::inflate::decompress_to_vec(raw).ok(),
                _ => None,
            };
        }
        at += 46 + name_len + extra_len + comment_len;
    }
    None
}

/// The names of the entries of a ZIP archive, from its central directory.
fn zip_names(archive: &[u8]) -> Vec<String> {
    let u16_at = |at: usize| u16::from_le_bytes([archive[at], archive[at + 1]]) as usize;
    let u32_at = |at: usize| u32::from_le_bytes(archive[at..at + 4].try_into().unwrap()) as usize;
    let Some(end) = (0..archive.len().saturating_sub(21))
        .rev()
        .find(|&at| archive[at..at + 4] == [0x50, 0x4b, 0x05, 0x06])
    else {
        return Vec::new();
    };
    let mut names = Vec::new();
    let mut at = u32_at(end + 16);
    for _ in 0..u16_at(end + 10) {
        let (name_len, extra_len, comment_len) =
            (u16_at(at + 28), u16_at(at + 30), u16_at(at + 32));
        names.push(String::from_utf8_lossy(&archive[at + 46..at + 46 + name_len]).into_owned());
        at += 46 + name_len + extra_len + comment_len;
    }
    names
}

#[test]
fn epub_navigation_labels_keep_a_non_breaking_hyphen() {
    let doc = doc_with_body(concat!(
        r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr>"#,
        r#"<w:r><w:t>Co</w:t><w:noBreakHyphen/><w:t>op members</w:t></w:r></w:p>"#,
    ));
    let epub = doc.to_epub_bytes().unwrap();
    let nav = String::from_utf8(
        zip_entry(&epub.bytes, "EPUB/nav.xhtml").expect("the EPUB holds a navigation document"),
    )
    .unwrap();
    assert!(nav.contains("Co-op members"), "{nav}");
}

#[test]
fn epub_draws_a_localised_heading_7_as_a_level_6_heading() {
    // Word's French "Titre7" is the built-in "heading 7": EPUB stops at h6.
    let mut doc = corpus_doc("word/report.docx");
    assert_eq!(
        doc.style("Titre7")
            .and_then(|style| style.name().map(str::to_owned))
            .as_deref(),
        Some("heading 7")
    );
    doc.add_paragraph("Deep heading").style("Titre7");
    let epub = doc.to_epub_bytes().unwrap();
    let chapters: String = zip_names(&epub.bytes)
        .iter()
        .filter(|name| name.ends_with(".xhtml") && !name.ends_with("nav.xhtml"))
        .filter_map(|name| zip_entry(&epub.bytes, name))
        .map(|bytes| String::from_utf8(bytes).unwrap())
        .collect();
    let at = chapters
        .find("Deep heading")
        .expect("the paragraph is exported");
    let open = chapters[..at].rfind('<').unwrap();
    let tag = &chapters[open..at];
    assert!(tag.starts_with("<h6"), "a level-6 heading, not {tag:?}");
}

/// The content documents of an EPUB but its navigation, joined.
fn epub_chapters(epub: &[u8]) -> String {
    zip_names(epub)
        .iter()
        .filter(|name| name.ends_with(".xhtml") && !name.ends_with("nav.xhtml"))
        .filter_map(|name| zip_entry(epub, name))
        .map(|bytes| String::from_utf8(bytes).unwrap())
        .collect()
}

/// The start tag of the first element whose text begins with `text`, a
/// document title left out.
fn start_tag_before<'a>(xhtml: &'a str, text: &str) -> &'a str {
    xhtml
        .match_indices(text)
        .map(|(at, _)| &xhtml[xhtml[..at].rfind('<').unwrap()..at])
        .find(|tag| !tag.starts_with("<title"))
        .unwrap_or_else(|| panic!("{text:?} is exported"))
}

#[test]
fn epub_keeps_the_heading_and_anchor_of_a_localised_heading_without_outline_level_in_a_list() {
    // A style made with StyleBuilder has no outline level, where Word writes
    // one into its built-in heading styles: the built-in name alone decides.
    let mut doc = Document::new();
    doc.add_style(StyleBuilder::paragraph("Titre2", "heading 2"))
        .unwrap();
    doc.add_paragraph("Before.");
    doc.add_numbered_list_item("Numbered heading", 0)
        .style("Titre2");
    doc.add_numbered_list_item("Numbered item", 0);
    let epub = doc.to_epub_bytes().unwrap();
    let chapters = epub_chapters(&epub.bytes);
    let tag = start_tag_before(&chapters, "Numbered heading");
    assert!(tag.starts_with("<h2"), "a level-2 heading, not {tag:?}");
    let anchor = tag
        .split("id=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap_or_else(|| panic!("the heading has an anchor: {tag:?}"));
    let nav = String::from_utf8(zip_entry(&epub.bytes, "EPUB/nav.xhtml").unwrap()).unwrap();
    let link = start_tag_before(&nav, "Numbered heading");
    assert!(
        link.contains(&format!("#{anchor}\"")),
        "the navigation links the heading: {link:?}\n{nav}"
    );
}

// ---- Styles ----

#[test]
fn a_new_document_defines_the_word_built_in_styles() {
    let mut doc = Document::new();
    assert_eq!(doc.add_common_styles().unwrap(), 0);
    let reopened = reopen(&mut doc);

    let normal = reopened.style("Normal").unwrap();
    assert!(normal.is_default());
    assert_eq!(normal.style_type(), StyleType::Paragraph);
    for level in 1..=9 {
        let heading = reopened.style(&format!("Heading{level}")).unwrap();
        assert_eq!(heading.name(), Some(format!("heading {level}").as_str()));
        assert_eq!(heading.based_on(), Some("Normal"));
        assert_eq!(heading.next_style(), Some("Normal"));
    }
    for (id, name) in [
        ("Title", "Title"),
        ("Subtitle", "Subtitle"),
        ("NoSpacing", "No Spacing"),
        ("Quote", "Quote"),
        ("ListParagraph", "List Paragraph"),
        ("Caption", "caption"),
    ] {
        assert_eq!(reopened.style(id).unwrap().name(), Some(name), "{id}");
    }
    assert_eq!(
        reopened
            .resolve_paragraph_properties(Some("ListParagraph"))
            .ind_left,
        Some(Twips(720))
    );
    assert_eq!(
        reopened.style("TableGrid").unwrap().style_type(),
        StyleType::Table
    );
    reopened.validate_style_graph().unwrap();
}

#[test]
fn a_custom_paragraph_style_applies_and_survives_a_save() {
    let mut doc = Document::new();
    doc.add_style(
        StyleBuilder::paragraph("Encadré", "Encadré 見出し")
            .based_on("Normal")
            .next_style("Normal")
            .priority(20)
            .quick_format(true)
            .alignment(Alignment::Center)
            .space_after(Length::pt(3.0))
            .indent_left(Length::inches(0.5))
            .size(13.0)
            .color("1F3864"),
    )
    .unwrap();
    doc.add_paragraph("Note").style("Encadré");

    let reopened = reopen(&mut doc);
    let style = reopened.style("Encadré").unwrap();
    assert_eq!(style.name(), Some("Encadré 見出し"));
    assert_eq!(style.style_type(), StyleType::Paragraph);
    assert_eq!(style.based_on(), Some("Normal"));
    assert_eq!(style.next_style(), Some("Normal"));
    assert_eq!(style.priority(), Some(20));
    assert_eq!(style.quick_format(), Some(true));
    assert!(!style.is_default());

    let paragraph = reopened.paragraph(0).unwrap();
    assert_eq!(paragraph.style_id(), Some("Encadré"));
    // Nothing is set on the paragraph itself: the style carries it all.
    assert_eq!(paragraph.alignment(), None);
    let effective = reopened.effective_paragraph_properties(&paragraph);
    assert_eq!(effective.jc, Some(ST_Jc::Center));
    assert_eq!(effective.space_after, Some(Twips(60)));
    assert_eq!(effective.ind_left, Some(Twips(720)));
    let run = reopened.effective_run_properties(&paragraph, &paragraph.run(0).unwrap());
    assert_eq!(run.sz.map(|size| size.to_pt()), Some(13.0));
    assert_eq!(run.color.as_deref(), Some("1F3864"));
}

#[test]
fn a_character_style_layers_over_the_paragraph_style() {
    let mut doc = Document::new();
    doc.add_style(
        StyleBuilder::paragraph("Big", "Big")
            .based_on("Normal")
            .size(14.0)
            .color("112233"),
    )
    .unwrap();
    doc.add_style(
        StyleBuilder::character("Loud", "Loud")
            .bold(true)
            .color("FF0000"),
    )
    .unwrap();
    let mut paragraph = doc.add_paragraph("").style("Big");
    paragraph.add_run("quiet ");
    paragraph.add_run("loud").style("Loud");

    let reopened = reopen(&mut doc);
    let paragraph = reopened.paragraph(0).unwrap();
    let quiet = paragraph.run(0).unwrap();
    let loud = paragraph.run(1).unwrap();
    assert_eq!(quiet.style_id(), None);
    assert_eq!(loud.style_id(), Some("Loud"));

    let quiet = reopened.effective_run_properties(&paragraph, &quiet);
    assert_eq!(quiet.sz.map(|size| size.to_pt()), Some(14.0));
    assert_eq!(quiet.bold, None);
    assert_eq!(quiet.color.as_deref(), Some("112233"));
    let loud = reopened.effective_run_properties(&paragraph, &loud);
    assert_eq!(loud.sz.map(|size| size.to_pt()), Some(14.0));
    assert_eq!(loud.bold, Some(true));
    assert_eq!(loud.color.as_deref(), Some("FF0000"));
    let resolved = reopened.resolve_run_properties(Some("Big"), Some("Loud"));
    assert_eq!(
        (resolved.sz, resolved.bold, resolved.color),
        (loud.sz, loud.bold, loud.color)
    );
}

#[test]
fn a_linked_pair_stays_while_anything_outside_it_names_either_side() {
    let mut doc = corpus_doc("word/report.docx");
    // Body paragraphs use Titre1, and the TOC heading style is based on it.
    let error = doc.remove_style("Titre1Car").unwrap_err();
    assert!(
        error.to_string().contains("'Titre1' is referenced by "),
        "{error}"
    );
    assert!(doc.style("Titre1Car").is_some());

    doc.add_style(StyleBuilder::character("Strong7", "Strong 7").based_on("Titre7Car"))
        .unwrap();
    let error = doc.remove_style("Titre7").unwrap_err();
    assert!(
        error
            .to_string()
            .contains("'Titre7Car' is referenced by style 'Strong7'"),
        "{error}"
    );
    assert!(doc.style("Titre7").is_some());
    assert!(doc.style("Titre7Car").is_some());
}

#[test]
fn a_style_that_links_to_another_goes_alone_when_the_other_does_not_link_back() {
    let mut doc = Document::new();
    doc.add_style(StyleBuilder::character("Kept", "Kept"))
        .unwrap();
    // The facade links both ways, so the one-way link is written as a file
    // could hold it.
    let mut package = OpcPackage::from_reader(Cursor::new(doc.to_bytes().unwrap())).unwrap();
    let styles = String::from_utf8(package.get_part("/word/styles.xml").unwrap().to_vec())
        .unwrap()
        .replace(
            "</w:styles>",
            r#"<w:style w:type="paragraph" w:styleId="OneWay"><w:name w:val="One Way"/><w:link w:val="Kept"/></w:style></w:styles>"#,
        );
    package.set_part("/word/styles.xml", styles.into_bytes());
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    let mut doc = Document::from_bytes(&out.into_inner()).unwrap();
    assert_eq!(doc.style("OneWay").unwrap().linked_style(), Some("Kept"));
    assert_eq!(doc.style("Kept").unwrap().linked_style(), None);
    // The one-way link is a use of Kept.
    let error = doc.remove_style("Kept").unwrap_err();
    assert!(
        error
            .to_string()
            .contains("'Kept' is referenced by style 'OneWay'"),
        "{error}"
    );
    assert!(doc.style("OneWay").is_some());
    assert!(doc.remove_style("OneWay").unwrap());
    assert!(doc.style("Kept").is_some());
}

#[test]
fn linking_a_paragraph_style_to_a_character_style_links_both_ways() {
    let mut doc = Document::new();
    doc.add_style(StyleBuilder::character("NoteChar", "Note Char").bold(true))
        .unwrap();
    doc.add_style(
        StyleBuilder::paragraph("Note", "Note")
            .based_on("Normal")
            .linked_style("NoteChar"),
    )
    .unwrap();
    assert_eq!(doc.style("NoteChar").unwrap().linked_style(), Some("Note"));
    let mut doc = reopen(&mut doc);
    doc.validate_style_graph().unwrap();
    assert_eq!(doc.style("Note").unwrap().linked_style(), Some("NoteChar"));
    assert_eq!(doc.style("NoteChar").unwrap().linked_style(), Some("Note"));
    // Cleared, the link no longer ties the two styles.
    doc.set_style(StyleBuilder::paragraph("Note", "Note").clear_linked_style())
        .unwrap();
    assert_eq!(doc.style("Note").unwrap().linked_style(), None);
    assert_eq!(doc.style("NoteChar").unwrap().linked_style(), None);
    assert!(doc.remove_style("NoteChar").unwrap());
    assert!(doc.remove_style("Note").unwrap());
    let reopened = reopen(&mut doc);
    assert!(reopened.style("Note").is_none());
    assert!(reopened.style("NoteChar").is_none());
}

#[test]
fn updating_a_style_changes_only_what_it_names() {
    let mut doc = Document::new();
    doc.add_style(
        StyleBuilder::paragraph("Memo", "Memo")
            .based_on("Normal")
            .space_after(Length::pt(3.0))
            .indent_left(Length::inches(0.5))
            .bold(true),
    )
    .unwrap();

    doc.set_style(StyleBuilder::paragraph("Memo", "Memo").alignment(Alignment::Right))
        .unwrap();
    let memo = doc.style("Memo").unwrap();
    assert_eq!(memo.based_on(), Some("Normal"));
    let properties = memo.paragraph_properties().unwrap();
    assert_eq!(
        (properties.jc, properties.space_after, properties.ind_left),
        (Some(ST_Jc::Right), Some(Twips(60)), Some(Twips(720)))
    );
    assert_eq!(memo.run_properties().unwrap().bold, Some(true));

    // A replacement drops what it does not carry, a clear drops one setting.
    doc.set_style(
        StyleBuilder::paragraph("Memo", "Memo")
            .replace_paragraph_properties(CT_PPr {
                ind_left: Some(Twips(100)),
                ..CT_PPr::default()
            })
            .clear_run_properties()
            .clear_based_on(),
    )
    .unwrap();
    let reopened = reopen(&mut doc);
    let memo = reopened.style("Memo").unwrap();
    assert_eq!(memo.based_on(), None);
    assert_eq!(memo.run_properties(), None);
    let properties = memo.paragraph_properties().unwrap();
    assert_eq!(
        (properties.jc, properties.space_after, properties.ind_left),
        (None, None, Some(Twips(100)))
    );

    let mut doc = reopened;
    assert!(
        doc.set_style(StyleBuilder::paragraph("Missing", "Missing"))
            .is_err()
    );
    let error = doc
        .set_style(StyleBuilder::character("Memo", "Memo"))
        .unwrap_err();
    assert!(
        error.to_string().contains("has type 'paragraph'"),
        "{error}"
    );
}

#[test]
fn a_style_in_use_cannot_be_removed_until_nothing_names_it() {
    let mut doc = Document::new();
    doc.add_style(StyleBuilder::paragraph("Used", "Used").based_on("Normal"))
        .unwrap();
    doc.add_style(StyleBuilder::paragraph("Spare", "Spare").based_on("Normal"))
        .unwrap();
    doc.add_paragraph("styled").style("Used");

    let error = doc.remove_style("Used").unwrap_err();
    assert!(error.to_string().contains("document content"), "{error}");
    assert!(doc.style("Used").is_some());
    assert!(doc.remove_style("Spare").unwrap());
    assert!(!doc.remove_style("Spare").unwrap());

    doc.paragraph_mut(0).unwrap().set_style_value(None);
    assert!(doc.remove_style("Used").unwrap());
    let reopened = reopen(&mut doc);
    assert!(reopened.style("Used").is_none());
    assert_eq!(reopened.paragraph(0).unwrap().style_id(), None);
}

#[test]
fn style_changes_that_break_the_style_graph_are_refused_and_change_nothing() {
    let mut doc = Document::new();
    doc.add_style(StyleBuilder::paragraph("Parent", "Parent").based_on("Normal"))
        .unwrap();
    doc.add_style(StyleBuilder::paragraph("Kid", "Kid").based_on("Parent"))
        .unwrap();
    let before = doc.to_bytes().unwrap();

    let refusals = [
        (
            doc.add_style(StyleBuilder::paragraph("Orphan", "Orphan").based_on("Nowhere")),
            "missing style 'Nowhere'",
        ),
        (
            doc.set_style(StyleBuilder::paragraph("Parent", "Parent").based_on("Kid")),
            "cycle",
        ),
        (
            doc.add_style(StyleBuilder::paragraph("Heading1", "Again")),
            "already exists",
        ),
        (
            doc.add_style(StyleBuilder::character("Shout", "Shout").alignment(Alignment::Center)),
            "cannot contain paragraph properties",
        ),
        (
            doc.add_style(StyleBuilder::paragraph("", "Nameless")),
            "cannot be empty",
        ),
        (
            doc.add_style(StyleBuilder::paragraph("Odd", "Odd").linked_style("Kid")),
            "cannot link to paragraph style",
        ),
    ];
    for (result, expected) in refusals {
        let error = result.expect_err(expected).to_string();
        assert!(error.contains(expected), "{error}");
    }
    assert_eq!(doc.to_bytes().unwrap(), before);
    doc.validate_style_graph().unwrap();
}

#[test]
fn the_default_paragraph_style_decides_how_unstyled_paragraphs_look() {
    let mut doc = Document::new();
    doc.add_style(
        StyleBuilder::paragraph("Body", "Body")
            .based_on("Normal")
            .alignment(Alignment::Justify),
    )
    .unwrap();
    doc.add_paragraph("unstyled");
    let error = doc
        .set_default_style(StyleType::Character, "Body")
        .unwrap_err();
    assert!(
        error.to_string().contains("has type 'paragraph'"),
        "{error}"
    );
    doc.set_default_style(StyleType::Paragraph, "Body").unwrap();

    let reopened = reopen(&mut doc);
    assert!(reopened.style("Body").unwrap().is_default());
    assert!(!reopened.style("Normal").unwrap().is_default());
    let paragraph = reopened.paragraph(0).unwrap();
    assert_eq!(paragraph.style_id(), None);
    assert_eq!(
        reopened.effective_paragraph_properties(&paragraph).jc,
        Some(ST_Jc::Both)
    );
    assert_eq!(
        reopened.resolve_paragraph_properties(None).jc,
        Some(ST_Jc::Both)
    );
    assert_eq!(
        reopened.resolve_paragraph_properties(Some("Normal")).jc,
        None
    );
}

// ---- Document defaults and effective properties ----

#[test]
fn document_defaults_set_the_font_size_and_spacing_of_plain_text() {
    let mut doc = Document::new();
    assert_eq!(doc.default_font_name(), Some("Calibri"));
    assert_eq!(doc.default_font_size().map(Length::to_pt), Some(11.0));
    doc.set_default_font_name_value(Some("Georgia")).unwrap();
    doc.set_default_font_size_value(Some(Length::pt(10.5)))
        .unwrap();
    doc.set_default_paragraph_properties(Some(CT_PPr {
        space_after: Some(Twips(0)),
        ..CT_PPr::default()
    }))
    .unwrap();
    assert!(
        doc.set_default_font_size_value(Some(Length::pt(-1.0)))
            .is_err()
    );
    doc.add_paragraph("plain");

    let mut reopened = reopen(&mut doc);
    assert_eq!(reopened.default_font_name(), Some("Georgia"));
    assert_eq!(reopened.default_font_size().map(Length::to_pt), Some(10.5));
    let paragraph = reopened.paragraph(0).unwrap();
    let run = reopened.effective_run_properties(&paragraph, &paragraph.run(0).unwrap());
    for font in [
        &run.font_ascii,
        &run.font_hansi,
        &run.font_east_asia,
        &run.font_cs,
    ] {
        assert_eq!(font.as_deref(), Some("Georgia"));
    }
    assert_eq!(run.sz.map(|size| size.to_pt()), Some(10.5));
    let spacing = reopened.effective_paragraph_properties(&paragraph);
    assert_eq!(spacing.space_after, Some(Twips(0)));
    // The replaced paragraph defaults no longer carry the 259 line spacing.
    assert_eq!(spacing.line_spacing, None);

    reopened.set_default_paragraph_properties(None).unwrap();
    let cleared = reopen(&mut reopened);
    assert!(cleared.default_paragraph_properties().is_none());
    assert_eq!(cleared.default_font_name(), Some("Georgia"));
}

#[test]
fn empty_paragraph_and_mark_properties_reach_their_written_form_in_one_save() {
    // Google Docs writes a paragraph mark with empty run properties. Each
    // form is written once as it stays: an edit pass run again on the saved
    // file changes no byte of the paragraphs it did not edit.
    let forms = [
        r#"<w:p><w:pPr><w:rPr/></w:pPr><w:r><w:t>x</w:t></w:r></w:p>"#,
        r#"<w:p><w:pPr><w:rPr></w:rPr></w:pPr><w:r><w:t>x</w:t></w:r></w:p>"#,
        r#"<w:p><w:pPr><w:rPr/></w:pPr></w:p>"#,
        r#"<w:p><w:pPr/></w:p>"#,
        r#"<w:p><w:pPr/><w:r><w:t>x</w:t></w:r></w:p>"#,
        r#"<w:p><w:pPr><w:jc w:val="center"/><w:rPr/></w:pPr><w:r><w:rPr/><w:t>x</w:t></w:r></w:p>"#,
    ];
    let main_xml = |bytes: &[u8]| {
        let package = OpcPackage::from_reader(Cursor::new(bytes.to_vec())).unwrap();
        let part = package.main_document_part().unwrap();
        String::from_utf8(package.get_part(&part).unwrap().to_vec()).unwrap()
    };
    for form in forms {
        let bytes = doc_with_body(&format!("{form}<w:p><w:r><w:t>H</w:t></w:r></w:p>"))
            .to_bytes()
            .unwrap();
        let pass = |bytes: &[u8], text: &str| {
            let mut doc = Document::from_bytes(bytes).unwrap();
            doc.paragraph_mut(1).unwrap().set_text(text).unwrap();
            doc.to_bytes().unwrap()
        };
        let once = pass(&bytes, "one");
        let again = pass(&pass(&once, "two"), "one");
        assert_eq!(main_xml(&again), main_xml(&once), "{form}");
    }
}

/// `doc` saved with the character styles a French Word file gives the
/// built-in Hyperlink and note mark styles, under their localized ids.
fn with_french_character_styles(doc: &mut Document) -> Document {
    let mut package = OpcPackage::from_reader(Cursor::new(doc.to_bytes().unwrap())).unwrap();
    let xml = String::from_utf8(package.get_part("/word/styles.xml").unwrap().to_vec()).unwrap();
    assert!(!xml.contains(r#"w:styleId="Hyperlink""#), "{xml}");
    let xml = xml.replace(
        "</w:styles>",
        concat!(
            r#"<w:style w:type="character" w:styleId="Lienhypertexte"><w:name w:val="Hyperlink"/>"#,
            r#"<w:rPr><w:color w:val="0000FF"/><w:u w:val="single"/></w:rPr></w:style>"#,
            r#"<w:style w:type="character" w:styleId="Appelnotedebasdep"><w:name w:val="footnote reference"/>"#,
            r#"<w:rPr><w:vertAlign w:val="superscript"/></w:rPr></w:style>"#,
            r#"<w:style w:type="character" w:styleId="Appeldenotedefin"><w:name w:val="endnote reference"/>"#,
            r#"<w:rPr><w:vertAlign w:val="superscript"/></w:rPr></w:style></w:styles>"#
        ),
    );
    package.set_part("/word/styles.xml", xml.into_bytes());
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    Document::from_bytes(&out.into_inner()).unwrap()
}

#[test]
fn links_and_note_marks_a_session_adds_take_a_localized_file_s_own_styles() {
    let mut doc = doc_with(&["body"]);
    doc.set_header("head");
    // A table in the header, for the table edit route.
    let mut package = OpcPackage::from_reader(Cursor::new(doc.to_bytes().unwrap())).unwrap();
    let header =
        String::from_utf8(package.get_part("/word/header1.xml").unwrap().to_vec()).unwrap();
    let header = header.replace(
        "</w:hdr>",
        concat!(
            r#"<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:tc>"#,
            r#"<w:p><w:r><w:t>cell</w:t></w:r></w:p></w:tc></w:tr></w:tbl><w:p/></w:hdr>"#
        ),
    );
    package.set_part("/word/header1.xml", header.into_bytes());
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    let mut doc = Document::from_bytes(&out.into_inner()).unwrap();
    let mut doc = with_french_character_styles(&mut doc);
    let relationship = doc.add_hyperlink_relationship("https://example.com/");
    let note = doc.add_footnote("note");
    {
        let mut paragraph = doc.paragraph_mut(0).unwrap();
        paragraph.add_hyperlink("site", &relationship);
        paragraph.add_internal_hyperlink("top", "_top", None);
        paragraph.add_footnote_ref(note);
    }
    let header = doc
        .stories()
        .unwrap()
        .into_iter()
        .find(|story| story.kind() == oxoffice_docx::StoryKind::Header)
        .unwrap();
    doc.add_hyperlink_to_story(&header, "head link", "https://example.com/head")
        .unwrap();
    // The routes Python's paragraph and table edits take in a header.
    let item = |doc: &Document, kind: oxoffice_docx::StoryItemKind, text: Option<&str>| {
        let header = doc
            .stories()
            .unwrap()
            .into_iter()
            .find(|story| story.kind() == oxoffice_docx::StoryKind::Header)
            .unwrap();
        doc.story_items(&header)
            .unwrap()
            .into_iter()
            .find(|item| item.kind() == kind && item.text().unwrap().as_deref() == text)
            .unwrap()
            .location()
            .clone()
    };
    let paragraph = item(&doc, oxoffice_docx::StoryItemKind::Paragraph, Some("head"));
    doc.edit_story_paragraph(&paragraph, |paragraph| {
        paragraph.add_internal_hyperlink("head top", "_top", None);
        Ok(())
    })
    .unwrap();
    // HTML inserted into the header.
    let paragraph = item(
        &doc,
        oxoffice_docx::StoryItemKind::Paragraph,
        Some("headhead top"),
    );
    doc.insert_html_fragment(
        &paragraph,
        r#"<p><a href="https://example.com/html">html link</a></p>"#,
        &[],
    )
    .unwrap();
    let table = item(&doc, oxoffice_docx::StoryItemKind::Table, None);
    doc.edit_story_table(&table, |table| {
        table
            .cell(0, 0)
            .unwrap()
            .paragraph_mut(0)
            .unwrap()
            .add_internal_hyperlink("cell top", "_top", None);
        Ok(())
    })
    .unwrap();

    let mut doc = reopen(&mut doc);
    let names = |name: &str| {
        doc.styles()
            .iter()
            .filter(|style| {
                style
                    .name()
                    .is_some_and(|value| value.eq_ignore_ascii_case(name))
            })
            .map(|style| style.style_id().to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(names("Hyperlink"), ["Lienhypertexte"]);
    assert_eq!(names("footnote reference"), ["Appelnotedebasdep"]);
    let bytes = doc.to_bytes().unwrap();
    let package = OpcPackage::from_reader(Cursor::new(bytes)).unwrap();
    for (part, count) in [("/word/document.xml", 3), ("/word/header1.xml", 4)] {
        let xml = String::from_utf8(package.get_part(part).unwrap().to_vec()).unwrap();
        assert!(!xml.contains(r#"w:val="Hyperlink""#), "{part}: {xml}");
        assert!(
            !xml.contains(r#"w:val="FootnoteReference""#),
            "{part}: {xml}"
        );
        let localized = xml.matches(r#"w:rStyle w:val="Lienhypertexte""#).count()
            + xml.matches(r#"w:rStyle w:val="Appelnotedebasdep""#).count();
        assert_eq!(localized, count, "{part}: {xml}");
    }
}

#[test]
fn a_localized_index_styled_as_hyperlinks_is_rebuilt_as_hyperlinks() {
    // The index's mark takes the French file's Hyperlink style, as Word
    // writes an index whose entries link to their pages.
    let mut doc = doc_with_body(concat!(
        r#"<w:p><w:r><w:t>Fruit</w:t></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r>"#,
        r#"<w:r><w:instrText xml:space="preserve"> XE "Apple" </w:instrText></w:r>"#,
        r#"<w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#,
        r#"<w:p><w:pPr><w:rPr><w:rStyle w:val="Lienhypertexte"/></w:rPr></w:pPr>"#,
        r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r>"#,
        r#"<w:r><w:instrText xml:space="preserve"> INDEX </w:instrText></w:r>"#,
        r#"<w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>old</w:t></w:r>"#,
        r#"<w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#,
    ));
    let mut doc = with_french_character_styles(&mut doc);
    let report = doc.rebuild_generated_tables().unwrap();
    assert_eq!(report.index_entries, 1, "{report:?}");

    let package = OpcPackage::from_reader(Cursor::new(doc.to_bytes().unwrap())).unwrap();
    let xml = String::from_utf8(package.get_part("/word/document.xml").unwrap().to_vec()).unwrap();
    let entry = xml.find(">Apple<").expect("the rebuilt entry");
    let run = xml[..entry].rfind("<w:r>").unwrap();
    assert!(
        xml[run..entry].contains(r#"<w:rStyle w:val="Lienhypertexte"/>"#),
        "{}",
        &xml[run..entry]
    );
    assert!(!xml.contains(r#"w:val="Hyperlink""#), "{xml}");
    let styles = String::from_utf8(package.get_part("/word/styles.xml").unwrap().to_vec()).unwrap();
    assert!(!styles.contains(r#"w:styleId="Hyperlink""#), "{styles}");
}

// ---- Styles in RTF ----

/// A document with a Heading 1 paragraph and one in a custom style based on
/// Normal whose runs take the theme's heading font.
fn doc_with_rtf_styles() -> Document {
    let mut doc = Document::new();
    let mut theme = oxoffice_drawing::theme::CT_OfficeStyleSheet::office_default();
    theme.theme_elements.font_scheme.major_font.latin.typeface = "Cambria".to_owned();
    doc.set_theme(theme).unwrap();
    let run_properties = oxoffice_docx::CT_RPr {
        font_ascii_theme: Some("majorHAnsi".to_owned()),
        italic: Some(true),
        ..Default::default()
    };
    doc.add_style(
        StyleBuilder::paragraph("QuoteBlock", "Quote Block")
            .based_on("Normal")
            .next_style("Normal")
            .indent_left(Length::twips(720))
            .run_properties(run_properties),
    )
    .unwrap();
    doc.add_paragraph("Results").set_style("Heading1");
    doc.add_paragraph("Body text");
    doc.add_paragraph("Quoted").set_style("QuoteBlock");
    doc
}

#[test]
fn rtf_export_lists_paragraph_styles_and_restates_their_formatting() {
    let doc = doc_with_rtf_styles();
    let written = doc.to_rtf_bytes().unwrap();
    assert!(
        !written
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("style")
                || diagnostic.message.contains("keep")
                || diagnostic.message.contains("theme")),
        "{:?}",
        written.diagnostics
    );
    let rtf = String::from_utf8(written.bytes).unwrap();
    let sheet_start = rtf.find(r"{\stylesheet").unwrap_or_else(|| panic!("{rtf}"));
    let sheet = &rtf[sheet_start..rtf[sheet_start..].find(r"}}\pard").unwrap() + sheet_start];
    // Normal first as \s0, each style after the one it is based on, with
    // its formatting resolved and the heading's keep-with-next.
    assert!(sheet.starts_with(r"{\stylesheet{\s0"), "{sheet}");
    assert!(sheet.contains(r" Normal;}"), "{sheet}");
    let heading = sheet
        .split(r"{\s")
        .find(|entry| entry.ends_with("heading 1;}") || entry.ends_with("heading 1;}}"))
        .unwrap_or_else(|| panic!("{sheet}"));
    assert!(
        heading.contains(r"\keepn") && heading.contains(r"\keep\"),
        "{heading}"
    );
    assert!(
        heading.contains(r"\outlinelevel0") && heading.contains(r"\b"),
        "{heading}"
    );
    assert!(heading.contains(r"\sbasedon0"), "{heading}");
    let quote = sheet
        .split(r"{\s")
        .find(|entry| entry.contains("Quote Block;"))
        .unwrap_or_else(|| panic!("{sheet}"));
    assert!(
        quote.contains(r"\li720") && quote.contains(r"\i"),
        "{quote}"
    );
    assert!(quote.contains(r"\sbasedon0\snext0"), "{quote}");
    // The theme's heading font is in the font table and on the style.
    let cambria = rtf
        .split(r"{\f")
        .find(|entry| entry.contains("Cambria;"))
        .and_then(|entry| entry.split(r"\fcharset").next())
        .unwrap_or_else(|| panic!("{rtf}"));
    assert!(quote.contains(&format!(r"\f{cambria}")), "{quote}");
    // Each paragraph names its style and repeats its formatting on its runs,
    // since `\plain` starts a run from nothing.
    let quote_number = quote.split(['\\', ' ']).next().unwrap();
    let paragraph = rtf
        .split(r"\pard")
        .find(|paragraph| paragraph.contains("Quoted"))
        .unwrap();
    assert!(
        paragraph.starts_with(&format!(r"\s{quote_number}\li720")),
        "{paragraph}"
    );
    assert!(
        paragraph.contains(&format!(r"{{\plain\f{cambria}")) && paragraph.contains(r"\i Quoted}"),
        "{paragraph}"
    );
}

#[test]
fn rtf_round_trip_keeps_paragraph_styles_without_direct_formatting() {
    let doc = doc_with_rtf_styles();
    let rtf = doc.to_rtf_bytes().unwrap().bytes;
    let mut read = Document::from_rtf_bytes(&rtf).unwrap().document;
    let styles: Vec<_> = read
        .paragraphs()
        .iter()
        .map(|paragraph| paragraph.style_id().map(str::to_owned))
        .collect();
    assert_eq!(
        styles,
        [
            Some("Heading1".to_owned()),
            None,
            Some("QuoteBlock".to_owned())
        ]
    );
    let quote = read.style("QuoteBlock").unwrap();
    assert_eq!(quote.name(), Some("Quote Block"));
    assert_eq!(quote.based_on(), Some("Normal"));
    assert_eq!(quote.next_style(), Some("Normal"));
    assert_eq!(quote.run_properties().unwrap().italic, Some(true));
    assert_eq!(
        quote.run_properties().unwrap().font_ascii.as_deref(),
        Some("Cambria")
    );
    let heading = read.style("Heading1").unwrap();
    assert_eq!(
        heading.paragraph_properties().unwrap().keep_next,
        Some(true)
    );
    assert_eq!(heading.paragraph_properties().unwrap().outline_lvl, Some(0));
    // What the paragraphs and runs restate from their style stays with it.
    let xml = reopen(&mut read).part_xml("word/document.xml").unwrap();
    for direct in [
        "<w:keepNext",
        "<w:i/>",
        "<w:b/>",
        "<w:ind ",
        "<w:outlineLvl",
        "<w:rFonts",
    ] {
        assert!(!xml.contains(direct), "{direct} in {xml}");
    }
}

#[test]
fn rtf_import_reads_the_paragraph_styles_of_a_word_stylesheet() {
    // As Word writes it: Normal without `\s0`, character and table styles
    // between the paragraph ones, a name outside ASCII, and every styled
    // paragraph and run restating its style's formatting.
    let rtf = br"{\rtf1\ansi\ansicpg1252\deff0{\fonttbl{\f0 Calibri;}{\f1 Cambria;}}
{\stylesheet{\ql \li0\ri0\sa80\widctlpar\fs20 \snext0 \sqformat \spriority0 Normal;}
{\s1\ql \li0\ri0\sb480\keep\keepn\widctlpar\outlinelevel0 \b\f1\fs28 \sbasedon0 \snext0 \slink20 \sqformat \spriority9 heading 1;}
{\*\cs10 \additive \ssemihidden Default Paragraph Font;}
{\*\cs20 \additive \b\f1\fs28 \sbasedon10 \slink1 Titre 1 Car;}
{\*\ts11\tsrowd\trftsWidthB3 \ql \li0\ri0\fs22 \snext11 \ssemihidden Normal Table;}
{\s23\ql \li567\ri0\sa80\widctlpar\i\fs20 \sbasedon0 \snext0 Citation \'e9tendue;}}
\pard\plain \ltrpar\s1\ql \li0\ri0\sb480\keep\keepn\widctlpar\outlinelevel0 \b\f1\fs28 {\insrsid1 Heading}{\b0 plain}\par
\pard\plain \ltrpar\ql \li0\ri0\sa80\widctlpar\fs20 {\insrsid1 Body}\par
\pard\plain \ltrpar\s23\ql \li567\ri0\sa80\widctlpar\i\fs20 {\insrsid1 Quote}\par}";
    let read = Document::from_rtf_bytes(rtf).unwrap();
    let mut doc = read.document;
    let styles: Vec<_> = doc
        .paragraphs()
        .iter()
        .map(|paragraph| paragraph.style_id().map(str::to_owned))
        .collect();
    assert_eq!(
        styles,
        [
            Some("Heading1".to_owned()),
            None,
            Some("CitationÉtendue".to_owned())
        ]
    );
    assert_eq!(
        doc.style("CitationÉtendue").unwrap().name(),
        Some("Citation étendue")
    );
    let normal = doc.style("Normal").unwrap();
    assert_eq!(
        normal.paragraph_properties().unwrap().space_after,
        Some(Twips(80))
    );
    let heading = doc.style("Heading1").unwrap();
    assert_eq!(heading.run_properties().unwrap().bold, Some(true));
    assert_eq!(
        heading.paragraph_properties().unwrap().keep_lines,
        Some(true)
    );
    // The run that turns bold off keeps that, and only that.
    let xml = reopen(&mut doc).part_xml("word/document.xml").unwrap();
    assert_eq!(xml.matches(r#"<w:b w:val="false"/>"#).count(), 1, "{xml}");
    for direct in [
        "<w:keepNext",
        "<w:b/>",
        "<w:i/>",
        "<w:spacing",
        "<w:sz ",
        "<w:ind ",
        "<w:jc ",
        "<w:rFonts",
        "<w:widowControl",
    ] {
        assert!(!xml.contains(direct), "{direct} in {xml}");
    }
}

#[test]
fn every_heading_of_the_word_paper_keeps_its_style_through_rtf() {
    let doc = corpus_doc("word/paper.docx");
    let headings = |doc: &Document| -> Vec<(String, String)> {
        doc.paragraphs()
            .iter()
            .filter(|paragraph| {
                paragraph
                    .style_id()
                    .and_then(|style| doc.style(style))
                    .is_some_and(|style| {
                        style.name().is_some_and(|name| name.starts_with("heading"))
                    })
            })
            .map(|paragraph| {
                let style = doc.style(paragraph.style_id().unwrap()).unwrap();
                (style.name().unwrap().to_owned(), paragraph.text())
            })
            .collect()
    };
    let expected = headings(&doc);
    assert!(expected.len() >= 4, "{expected:?}");
    let rtf = doc.to_rtf_bytes().unwrap().bytes;
    let read = Document::from_rtf_bytes(&rtf).unwrap().document;
    assert_eq!(headings(&read), expected);
}
