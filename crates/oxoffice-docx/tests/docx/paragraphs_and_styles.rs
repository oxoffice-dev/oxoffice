//! Paragraphs and styles: the order of body content (insert, move, copy,
//! remove), paragraph formatting, headings, paragraph and character styles,
//! document defaults and the properties a paragraph or run ends up with.

use oxoffice_docx::{
    Alignment, BodyItemRef, BorderStyle, CT_PPr, ContentLocation, Document, Length,
    ParagraphBorderEdge, StoryId, StoryKind, StyleBuilder, StyleType, TabAlignment, TabLeader,
    Twips,
};
use oxoffice_docx_oxml::shared::ST_Jc;

use crate::support::{corpus_doc, doc_with, reopen, texts};

/// The main body story of `doc`, as the story operations want it.
fn body_story(doc: &Document) -> StoryId {
    doc.stories()
        .expect("stories are listed")
        .into_iter()
        .find(|story| story.kind() == StoryKind::Body)
        .expect("a body story")
}

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

/// Index of the paragraph whose text is `text`.
fn paragraph_index(doc: &Document, text: &str) -> usize {
    texts(doc)
        .iter()
        .position(|candidate| candidate == text)
        .unwrap_or_else(|| panic!("no paragraph reads {text:?}"))
}

/// The six headings of the corpus report, as (level, text).
fn report_headings() -> Vec<(u32, String)> {
    [
        (1, "1. Highlights"),
        (1, "2. Financial overview"),
        (1, "3. Operations"),
        (2, "3.1 Support"),
        (2, "3.2 Next steps"),
        (1, "Appendix A. Regional detail"),
    ]
    .into_iter()
    .map(|(level, text)| (level, text.to_owned()))
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
fn a_paragraph_moves_forward_and_back_without_disturbing_the_others() {
    let mut doc = doc_with(&["A", "B", "C", "D"]);
    let b = paragraph_location(&doc, 1);
    doc.move_content(&b, &ContentLocation::end(body_story(&doc)))
        .unwrap();
    assert_eq!(texts(&doc), ["A", "C", "D", "B"]);

    let d = paragraph_location(&doc, 2);
    let front = paragraph_location(&doc, 0);
    doc.move_content(&d, &front).unwrap();
    assert_eq!(texts(&doc), ["D", "A", "C", "B"]);

    // Moving a paragraph in front of itself is a no-op.
    let a = paragraph_location(&doc, 1);
    doc.move_content(&a, &a.clone()).unwrap();
    assert_eq!(texts(&reopen(&mut doc)), ["D", "A", "C", "B"]);
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

#[test]
fn a_paragraph_is_copied_or_cut_and_pasted_with_its_formatting() {
    let mut doc = doc_with(&["intro"]);
    let mut heading = doc
        .add_paragraph("")
        .style("Heading2")
        .alignment(Alignment::Center);
    heading.add_run("Key ").bold(true);
    heading.add_run("figures");
    doc.add_paragraph("outro");

    let source = paragraph_location(&doc, 1);
    doc.clone_content(&source, &ContentLocation::end(body_story(&doc)))
        .unwrap();
    // The copy is a paragraph of its own: editing it leaves the original as it was.
    doc.paragraph_mut(3)
        .unwrap()
        .set_text("Copied figures")
        .unwrap();

    // Cut the outro and paste it at the top.
    let outro = doc.remove_content_at(&paragraph_location(&doc, 2)).unwrap();
    doc.insert_content(&paragraph_location(&doc, 0), outro)
        .unwrap();

    let reopened = reopen(&mut doc);
    assert_eq!(
        texts(&reopened),
        ["outro", "intro", "Key figures", "Copied figures"]
    );
    let original = reopened.paragraph(2).unwrap();
    let copy = reopened.paragraph(3).unwrap();
    for paragraph in [&original, &copy] {
        assert_eq!(paragraph.style_id(), Some("Heading2"));
        assert_eq!(paragraph.alignment(), Some(Alignment::Center));
    }
    assert!(original.run(0).unwrap().is_bold());
    assert_eq!(
        reopened.headings(),
        [
            (2, "Key figures".to_owned()),
            (2, "Copied figures".to_owned())
        ]
    );
}

#[test]
fn editing_the_word_report_keeps_its_runs_and_unique_paragraph_ids() {
    // Word splits this paragraph into five runs.
    let mut doc = corpus_doc("word/report.docx");
    let support = paragraph_index(&doc, "3.1 Support");
    let median_text = doc.paragraph(support + 1).unwrap().text();
    assert_eq!(doc.paragraph(support + 1).unwrap().run_count(), 5);

    doc.paragraph_mut(support).unwrap().set_style("Titre3");
    doc.paragraph_mut(support + 1)
        .unwrap()
        .set_alignment(Alignment::Justify);
    let median = paragraph_location(&doc, support + 1);
    doc.clone_content(&median, &ContentLocation::end(body_story(&doc)))
        .unwrap();

    let reopened = reopen(&mut doc);
    assert_eq!(
        reopened.paragraph(support).unwrap().style_id(),
        Some("Titre3")
    );
    let edited = reopened.paragraph(support + 1).unwrap();
    assert_eq!(edited.alignment(), Some(Alignment::Justify));
    assert_eq!(edited.run_count(), 5);
    assert_eq!(edited.text(), median_text);
    let copy = reopened.paragraph(reopened.paragraph_count() - 1).unwrap();
    assert_eq!(copy.text(), median_text);
    assert_eq!(copy.run_count(), 5);

    // The copy does not repeat the w14:paraId Word gave the original.
    let xml = reopened.part_xml("word/document.xml").unwrap();
    let mut ids: Vec<&str> = xml
        .split("w14:paraId=\"")
        .skip(1)
        .map(|rest| &rest[..rest.find('"').unwrap()])
        .collect();
    let count = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), count, "a w14:paraId is repeated");
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

// Fails today: set_first_line_indent leaves an earlier hanging indent in w:ind, and Word applies the hanging one. Issue: TBD
#[test]
#[should_panic(expected = "the first-line indent replaces the hanging indent")]
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

// Fails today: headings() only knows the English style IDs, so a French Word file (Titre1, Titre2) has none. Issue: TBD
#[test]
#[should_panic(expected = "every heading of the Word report is found")]
fn headings_are_found_in_the_word_and_google_reports_alike() {
    assert_eq!(
        corpus_doc("google/report.docx").headings(),
        report_headings(),
        "every heading of the Google report is found"
    );
    // Word for Mac in French names the styles Titre1 and Titre2, with the
    // built-in names "heading 1" and "heading 2".
    let word = corpus_doc("word/report.docx");
    assert_eq!(word.style("Titre1").unwrap().name(), Some("heading 1"));
    assert_eq!(
        word.headings(),
        report_headings(),
        "every heading of the Word report is found"
    );
}

// Fails today: Heading1 of a new document has no w:outlineLvl, where Word writes 0 and Heading2 to 9 carry theirs. Issue: TBD
#[test]
#[should_panic(expected = "Heading 1 has outline level 0")]
fn heading_1_of_a_new_document_has_an_outline_level_like_the_others() {
    let doc = Document::new();
    for level in 2..=9 {
        assert_eq!(
            doc.resolve_paragraph_properties(Some(&format!("Heading{level}")))
                .outline_lvl,
            Some(level - 1)
        );
    }
    assert_eq!(
        doc.resolve_paragraph_properties(Some("Heading1"))
            .outline_lvl,
        Some(0),
        "Heading 1 has outline level 0"
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
fn a_style_inherits_from_its_base_and_follows_later_changes_to_it() {
    let mut doc = Document::new();
    doc.add_style(
        StyleBuilder::paragraph("Base", "Base")
            .based_on("Normal")
            .alignment(Alignment::Center)
            .space_after(Length::pt(4.0)),
    )
    .unwrap();
    doc.add_style(
        StyleBuilder::paragraph("Child", "Child")
            .based_on("Base")
            .alignment(Alignment::Right),
    )
    .unwrap();

    let child = doc.resolve_paragraph_properties(Some("Child"));
    assert_eq!(child.jc, Some(ST_Jc::Right));
    assert_eq!(child.space_after, Some(Twips(80)));
    // The line spacing comes from the document defaults, under Normal.
    assert_eq!(child.line_spacing, Some(Twips(259)));

    doc.set_style(StyleBuilder::paragraph("Base", "Base").space_after(Length::pt(9.0)))
        .unwrap();
    let mut doc = reopen(&mut doc);
    assert_eq!(
        doc.resolve_paragraph_properties(Some("Child")).space_after,
        Some(Twips(180))
    );
    assert_eq!(
        doc.resolve_paragraph_properties(Some("Child")).jc,
        Some(ST_Jc::Right)
    );
    let error = doc.remove_style("Base").unwrap_err();
    assert!(error.to_string().contains("'Child'"), "{error}");
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
    // A linked style cannot go while its partner names it.
    assert!(doc.remove_style("NoteChar").is_err());

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

#[test]
fn a_paragraph_naming_an_undefined_style_is_reported_by_validation() {
    let mut doc = Document::new();
    doc.add_paragraph("lost style").style("DoesNotExist");
    let report = doc.validate().unwrap();
    assert!(!report.is_valid());
    assert!(
        report
            .findings
            .iter()
            .any(|finding| finding.message.contains("\"DoesNotExist\"")),
        "{:?}",
        report.findings
    );
    // The file still saves: Word shows such a paragraph in the default style.
    let reopened = reopen(&mut doc);
    assert_eq!(
        reopened.paragraph(0).unwrap().style_id(),
        Some("DoesNotExist")
    );
}

#[test]
fn common_styles_fill_in_only_what_each_producer_left_out() {
    // Google Docs defines Heading1 to Heading6, Title and Subtitle, and a
    // table style of its own for each table.
    let mut google = corpus_doc("google/report.docx");
    assert!(google.style("Heading7").is_none());
    assert_eq!(google.add_common_styles().unwrap(), 8);
    for id in [
        "Heading7",
        "Heading8",
        "Heading9",
        "NoSpacing",
        "Quote",
        "ListParagraph",
        "Caption",
        "TableGrid",
    ] {
        assert!(google.style(id).is_some(), "{id}");
    }
    assert!(google.style("Table1").is_some());
    assert_eq!(google.style("Normal").unwrap().name(), Some("normal"));
    assert_eq!(google.add_common_styles().unwrap(), 0);
    let reopened = reopen(&mut google);
    reopened.validate_style_graph().unwrap();
    assert!(reopened.style("Heading9").is_some());

    // Word in French already has them all under localized IDs, matched by name.
    let mut word = corpus_doc("word/report.docx");
    let count = word.styles().len();
    assert_eq!(word.add_common_styles().unwrap(), 0);
    assert_eq!(word.styles().len(), count);
    assert!(word.style("Heading2").is_none());
}

#[test]
fn a_defect_the_producer_left_in_the_styles_does_not_block_new_styles() {
    // LibreOffice links BodyText to BodyTextChar, which does not link back.
    let mut doc = corpus_doc("libreoffice/report.docx");
    let error = doc.validate_style_graph().unwrap_err();
    assert!(error.to_string().contains("'BodyText'"), "{error}");

    doc.add_style(StyleBuilder::paragraph("Mine", "Mine").based_on("Normal"))
        .unwrap();
    assert!(
        doc.add_style(StyleBuilder::paragraph("Broken", "Broken").based_on("Nowhere"))
            .is_err()
    );
    let reopened = reopen(&mut doc);
    assert!(reopened.style("Mine").is_some());
    assert!(reopened.style("Broken").is_none());
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
fn the_default_font_yields_to_a_font_the_normal_style_names() {
    // The Word report names Calibri on Normal and a theme font in the defaults.
    let mut doc = corpus_doc("word/report.docx");
    assert_eq!(doc.default_font_name(), None);
    assert_eq!(
        doc.style("Normal")
            .unwrap()
            .run_properties()
            .and_then(|properties| properties.font_ascii.as_deref()),
        Some("Calibri")
    );
    doc.set_default_font_name_value(Some("Arial")).unwrap();

    let reopened = reopen(&mut doc);
    assert_eq!(reopened.default_font_name(), Some("Arial"));
    let summary = reopened.paragraph(1).unwrap();
    assert!(summary.text().starts_with("Summary."));
    let run = reopened.effective_run_properties(&summary, &summary.run(0).unwrap());
    assert_eq!(run.font_ascii.as_deref(), Some("Calibri"));
}

#[test]
fn effective_paragraph_properties_layer_direct_over_style_over_defaults() {
    let mut doc = Document::new();
    doc.add_style(
        StyleBuilder::paragraph("Spaced", "Spaced")
            .based_on("Normal")
            .alignment(Alignment::Center)
            .space_after(Length::pt(3.0)),
    )
    .unwrap();
    doc.add_paragraph("layered")
        .style("Spaced")
        .space_after(Length::pt(6.0))
        .alignment(Alignment::Right);

    let mut reopened = reopen(&mut doc);
    let paragraph = reopened.paragraph(0).unwrap();
    let effective = reopened.effective_paragraph_properties(&paragraph);
    assert_eq!(effective.jc, Some(ST_Jc::Right), "direct wins");
    assert_eq!(effective.space_after, Some(Twips(120)), "direct wins");
    assert_eq!(
        effective.line_spacing,
        Some(Twips(259)),
        "defaults fill the rest"
    );

    // Clearing direct formatting falls back to what the style says.
    let mut paragraph = reopened.paragraph_mut(0).unwrap();
    paragraph.set_alignment_value(None);
    paragraph.set_space_after_value(None);
    let cleared = reopen(&mut reopened);
    let paragraph = cleared.paragraph(0).unwrap();
    assert_eq!(paragraph.alignment(), None);
    let effective = cleared.effective_paragraph_properties(&paragraph);
    assert_eq!(effective.space_after, Some(Twips(60)));
    assert_eq!(effective.jc, Some(ST_Jc::Center));
}

#[test]
fn direct_formatting_google_writes_on_list_items_wins_over_the_defaults() {
    // Google Docs writes spacing, indents and alignment on every list item.
    let doc = corpus_doc("google/report.docx");
    assert_eq!(
        doc.default_paragraph_properties()
            .and_then(|properties| properties.space_after),
        Some(Twips(200))
    );
    let index = paragraph_index(
        &doc,
        "Subscription revenue reached 4.2 million euros, up from 3.7 million.",
    );
    let item = doc.paragraph(index).unwrap();
    assert_eq!(item.style_id(), None);
    assert!(item.numbering().is_some());
    let effective = doc.effective_paragraph_properties(&item);
    assert_eq!(effective.space_after, Some(Twips(0)));
    assert_eq!(effective.ind_left, Some(Twips(360)));
    assert_eq!(effective.ind_hanging, Some(Twips(360)));
    assert_eq!(twips(item.first_line_indent()), Some(-360));
    assert_eq!(item.alignment(), Some(Alignment::Left));
}

// Fails today: has_borders() counts the nil edges Google Docs writes on every list item as borders. Issue: TBD
#[test]
#[should_panic(expected = "a nil edge draws no border")]
fn a_google_list_item_whose_edges_are_all_nil_has_no_borders() {
    let doc = corpus_doc("google/report.docx");
    let index = paragraph_index(
        &doc,
        "Subscription revenue reached 4.2 million euros, up from 3.7 million.",
    );
    let item = doc.paragraph(index).unwrap();
    for edge in [
        ParagraphBorderEdge::Top,
        ParagraphBorderEdge::Left,
        ParagraphBorderEdge::Bottom,
        ParagraphBorderEdge::Right,
        ParagraphBorderEdge::Between,
    ] {
        assert_eq!(item.border(edge).map(|border| border.style()), Some("nil"));
    }
    assert!(!item.has_borders(), "a nil edge draws no border");
}
