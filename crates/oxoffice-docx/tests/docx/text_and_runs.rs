//! Runs and their text: run formatting, adding, editing, splitting and
//! removing runs, breaks and whitespace, unicode text, find and replace, and
//! hyperlinks read and edited as text.

use std::collections::HashMap;

use oxoffice_docx::{
    BreakKind, Document, FieldEvaluationContext, HdrFtrType, ParagraphItemRef, ParagraphRef,
    ReplaceOptions, ReplacementCountMismatch, RunFontSlot, RunItemRef, StoryItemKind, StoryKind,
    UnderlineStyle,
};

use crate::support::{corpus_doc, doc_with, doc_with_body, reopen, texts};

/// The texts of the runs of body paragraph `index`, in order.
fn run_texts(doc: &Document, index: usize) -> Vec<String> {
    let paragraph = doc.paragraph(index).expect("paragraph exists");
    paragraph.runs().map(|run| run.text()).collect()
}

/// The text items (text, tabs and breaks) of run `run` of `paragraph`, as
/// short labels.
fn run_items(paragraph: &ParagraphRef<'_>, run: usize) -> Vec<String> {
    paragraph
        .run(run)
        .expect("run exists")
        .items()
        .filter_map(|item| match item {
            RunItemRef::Text(text) => Some(format!("text:{text}")),
            RunItemRef::Tab => Some("tab".to_owned()),
            RunItemRef::Break(kind) => Some(format!("break:{kind:?}")),
            _ => None,
        })
        .collect()
}

// ---- Run formatting ----

#[test]
fn formatted_runs_keep_their_text_and_order_after_a_save() {
    let mut doc = Document::new();
    let mut paragraph = doc.add_paragraph("");
    paragraph.add_run("This is ");
    paragraph.add_run("bold").bold(true);
    paragraph.add_run(" and ");
    paragraph.add_run("italic").italic(true);
    paragraph.add_run(".");

    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["This is bold and italic."]);
    assert_eq!(
        run_texts(&doc, 0),
        ["This is ", "bold", " and ", "italic", "."]
    );
    let paragraph = doc.paragraph(0).unwrap();
    let bold: Vec<bool> = paragraph.runs().map(|run| run.is_bold()).collect();
    let italic: Vec<bool> = paragraph.runs().map(|run| run.is_italic()).collect();
    assert_eq!(bold, [false, true, false, false, false]);
    assert_eq!(italic, [false, false, false, true, false]);
}

#[test]
fn character_toggles_survive_a_save() {
    let mut doc = Document::new();
    let mut paragraph = doc.add_paragraph("");
    paragraph.add_run("bold").bold(true);
    paragraph.add_run("italic").italic(true);
    paragraph.add_run("underline").underline(true);
    paragraph.add_run("strike").strike(true);
    paragraph.add_run("double strike").double_strike(true);
    paragraph.add_run("caps").all_caps(true);
    paragraph.add_run("small caps").small_caps(true);
    paragraph.add_run("hidden").hidden(true);

    let doc = reopen(&mut doc);
    let paragraph = doc.paragraph(0).unwrap();
    let run = |index| paragraph.run(index).unwrap();
    assert_eq!(run(0).bold_value(), Some(true));
    assert_eq!(run(1).italic_value(), Some(true));
    assert!(run(2).is_underline());
    assert_eq!(run(3).strike_value(), Some(true));
    assert_eq!(run(4).double_strike_value(), Some(true));
    assert_eq!(run(5).all_caps_value(), Some(true));
    assert_eq!(run(6).small_caps_value(), Some(true));
    assert_eq!(run(7).hidden_value(), Some(true));
    // All caps is a display property: the stored text keeps its case.
    assert_eq!(run(5).text(), "caps");
    // No toggle leaks to a neighbour.
    assert_eq!(run(1).bold_value(), None);
    assert_eq!(run(0).italic_value(), None);
}

#[test]
fn colour_highlight_size_and_font_survive_a_save_and_unknown_highlights_are_refused() {
    let mut doc = Document::new();
    let mut paragraph = doc.add_paragraph("");
    let mut run = paragraph
        .add_run("styled")
        .color("C00000")
        .size(14.5)
        .font("Georgia");
    assert!(run.set_highlight_value(Some("yellow")));
    paragraph.add_run("shaded").highlight("FFFF00");
    let mut unknown = paragraph.add_run("unknown");
    assert!(!unknown.set_highlight_value(Some("chartreuse")));

    let doc = reopen(&mut doc);
    let paragraph = doc.paragraph(0).unwrap();
    let styled = paragraph.run(0).unwrap();
    assert_eq!(styled.color(), Some("C00000"));
    assert_eq!(styled.size(), Some(14.5));
    assert_eq!(styled.font_name(), Some("Georgia"));
    assert_eq!(styled.highlight_color(), Some("yellow"));
    // The `highlight` builder writes a shading fill, not a Word highlight.
    let shaded = paragraph.run(1).unwrap();
    assert_eq!(shaded.highlight_color(), None);
    assert_eq!(shaded.shading_fill(), Some("FFFF00"));
    assert_eq!(shaded.highlight().as_deref(), Some("FFFF00"));
    // A name Word does not know is refused, not written.
    assert_eq!(paragraph.run(2).unwrap().highlight(), None);
}

#[test]
fn superscript_subscript_and_underline_styles_survive_a_save() {
    let mut doc = Document::new();
    let mut paragraph = doc.add_paragraph("");
    paragraph.add_run("H");
    paragraph.add_run("2").subscript();
    paragraph.add_run("O x");
    paragraph.add_run("2").superscript();
    paragraph
        .add_run("double")
        .underline_style(UnderlineStyle::Double);
    paragraph
        .add_run("wave")
        .underline_style(UnderlineStyle::Wave);

    let doc = reopen(&mut doc);
    let paragraph = doc.paragraph(0).unwrap();
    assert_eq!(paragraph.text(), "H2O x2doublewave");
    assert_eq!(paragraph.run(1).unwrap().vert_align(), Some("subscript"));
    assert_eq!(paragraph.run(3).unwrap().vert_align(), Some("superscript"));
    assert_eq!(paragraph.run(0).unwrap().vert_align(), None);
    // Codes are the ones language bindings use: 3 double, 11 wave.
    assert_eq!(paragraph.run(4).unwrap().underline_code_value(), Some(3));
    assert_eq!(paragraph.run(5).unwrap().underline_code_value(), Some(11));
}

#[test]
fn heavy_and_long_underline_styles_are_read_and_kept() {
    // The heavy and long values of ST_Underline, with the WdUnderline code
    // bindings use.
    for (value, code) in [
        ("dottedHeavy", 20),
        ("dashedHeavy", 23),
        ("dashDotHeavy", 25),
        ("dashDotDotHeavy", 26),
        ("wavyHeavy", 27),
        ("dashLong", 39),
        ("wavyDouble", 43),
        ("dashLongHeavy", 55),
    ] {
        let mut doc = doc_with_body(&format!(
            r#"<w:p><w:r><w:rPr><w:u w:val="{value}"/></w:rPr><w:t>styled</w:t></w:r></w:p>"#
        ));
        let paragraph = doc.paragraph(0).unwrap();
        let run = paragraph.run(0).unwrap();
        assert!(run.is_underline(), "{value}");
        assert_eq!(run.underline_code_value(), Some(code), "{value}");
        assert!(doc.to_html_fragment().contains("<u>"), "{value} in HTML");
        let reopened = reopen(&mut doc);
        let xml = reopened.part_xml("word/document.xml").unwrap();
        assert!(
            xml.contains(&format!(r#"w:u w:val="{value}""#)),
            "{value}: {xml}"
        );
    }
    // Through the typed style.
    let mut doc = Document::new();
    doc.add_paragraph("")
        .add_run("double wave")
        .underline_style(UnderlineStyle::WavyDouble);
    let reopened = reopen(&mut doc);
    assert_eq!(
        reopened
            .paragraph(0)
            .unwrap()
            .run(0)
            .unwrap()
            .underline_code_value(),
        Some(43)
    );
    // And through the binding's code.
    let mut doc = Document::new();
    doc.add_paragraph("").add_run("long");
    doc.paragraph_mut(0)
        .unwrap()
        .run_mut(0)
        .unwrap()
        .set_underline_code_value(Some(39));
    let reopened = reopen(&mut doc);
    assert!(
        reopened
            .part_xml("word/document.xml")
            .unwrap()
            .contains(r#"w:u w:val="dashLong""#)
    );
}

#[test]
fn clearing_a_toggle_differs_from_turning_it_off() {
    let mut doc = Document::new();
    let mut paragraph = doc.add_paragraph("");
    paragraph.add_run("off").bold(true).set_bold(false);
    let mut cleared = paragraph.add_run("cleared").bold(true).italic(true);
    cleared.set_bold_value(None);
    cleared.set_italic_value(None);

    let doc = reopen(&mut doc);
    let paragraph = doc.paragraph(0).unwrap();
    // An explicit "off" overrides a bold style; a cleared value inherits it.
    assert_eq!(paragraph.run(0).unwrap().bold_value(), Some(false));
    assert_eq!(paragraph.run(1).unwrap().bold_value(), None);
    assert_eq!(paragraph.run(1).unwrap().italic_value(), None);
}

// ---- Adding, editing, splitting and removing runs ----

#[test]
fn editing_run_text_keeps_its_format() {
    let mut doc = Document::new();
    let mut paragraph = doc.add_paragraph("");
    paragraph.add_run("Total: ");
    paragraph.add_run("12").bold(true).color("FF0000");

    let mut doc = reopen(&mut doc);
    doc.paragraph_mut(0)
        .unwrap()
        .run_mut(1)
        .unwrap()
        .set_text("1,250");
    doc.paragraph_mut(0)
        .unwrap()
        .run_mut(0)
        .unwrap()
        .add_text("EUR ");

    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["Total: EUR 1,250"]);
    let paragraph = doc.paragraph(0).unwrap();
    let edited = paragraph.run(1).unwrap();
    assert_eq!(edited.bold_value(), Some(true));
    assert_eq!(edited.color(), Some("FF0000"));
}

#[test]
fn splitting_a_run_gives_two_runs_with_the_same_format() {
    let mut doc = Document::new();
    doc.add_paragraph("")
        .add_run("naïve 😀 world")
        .italic(true)
        .font("Arial");

    // The offset counts characters: "naïve 😀" is 7 characters, 11 bytes.
    let boundary = doc.paragraph_mut(0).unwrap().split_run(0, 7).unwrap();
    assert_eq!(boundary, 1);
    let doc = reopen(&mut doc);
    assert_eq!(run_texts(&doc, 0), ["naïve 😀", " world"]);
    let paragraph = doc.paragraph(0).unwrap();
    for run in paragraph.runs() {
        assert_eq!(run.italic_value(), Some(true), "{:?}", run.text());
        assert_eq!(run.font_name(), Some("Arial"), "{:?}", run.text());
    }
}

#[test]
fn splitting_at_a_run_edge_or_outside_it_changes_nothing() {
    let mut doc = doc_with(&["abc"]);
    let before = doc.to_bytes().unwrap();
    // The edges give the boundary before and after the run.
    assert_eq!(doc.split_run(0, 0, 0).unwrap(), 0);
    assert_eq!(doc.split_run(0, 0, 3).unwrap(), 1);
    // Body index, run index and character offset each out of range.
    assert!(doc.split_run(1, 0, 1).is_err());
    assert!(doc.split_run(0, 1, 1).is_err());
    assert!(doc.split_run(0, 0, 4).is_err());
    assert_eq!(doc.to_bytes().unwrap(), before);
}

#[test]
fn removing_a_run_drops_only_its_text() {
    let mut doc = Document::new();
    let mut paragraph = doc.add_paragraph("");
    paragraph.add_run("keep ");
    paragraph.add_run("drop ").bold(true);
    paragraph.add_run("keep");

    doc.paragraph_mut(0).unwrap().remove_run(1).unwrap();
    assert!(doc.paragraph_mut(0).unwrap().remove_run(5).is_err());
    let doc = reopen(&mut doc);
    assert_eq!(run_texts(&doc, 0), ["keep ", "keep"]);
}

// ---- Breaks, tabs and whitespace ----

#[test]
fn tabs_and_breaks_read_back_in_place_as_items_and_text() {
    let mut doc = Document::new();
    let mut paragraph = doc.add_paragraph("");
    let mut run = paragraph.add_run("Name");
    run.add_tab();
    run.add_text("Ada");
    run.add_break(BreakKind::Line);
    run.add_text("next line");
    run.add_break(BreakKind::Page);
    run.add_text("next page");

    let doc = reopen(&mut doc);
    let paragraph = doc.paragraph(0).unwrap();
    assert_eq!(
        run_items(&paragraph, 0),
        [
            "text:Name",
            "tab",
            "text:Ada",
            "break:Line",
            "text:next line",
            "break:Page",
            "text:next page",
        ]
    );
    // Text reading shows a tab as a tab and every break as a line feed.
    assert_eq!(paragraph.text(), "Name\tAda\nnext line\nnext page");
}

#[test]
fn paragraph_text_setter_turns_tabs_and_line_feeds_into_tab_and_break_items() {
    let mut doc = doc_with(&["old"]);
    doc.paragraph_mut(0).unwrap().set_text("a\tb\nc").unwrap();
    let doc = reopen(&mut doc);
    let paragraph = doc.paragraph(0).unwrap();
    assert_eq!(paragraph.run_count(), 1);
    assert_eq!(
        run_items(&paragraph, 0),
        ["text:a", "tab", "text:b", "break:Line", "text:c"]
    );
}

#[test]
fn paragraph_text_setter_turns_a_windows_line_ending_into_one_break() {
    let mut doc = doc_with(&["old"]);
    doc.paragraph_mut(0).unwrap().set_text("c\r\nd").unwrap();
    let doc = reopen(&mut doc);
    let paragraph = doc.paragraph(0).unwrap();
    assert_eq!(
        run_items(&paragraph, 0),
        ["text:c", "break:Line", "text:d"],
        "a Windows line ending is one break"
    );
}

#[test]
fn story_text_turns_a_windows_line_ending_into_one_break() {
    let mut doc = doc_with(&["old"]);
    let location = doc
        .story_items(&doc.stories().unwrap()[0])
        .unwrap()
        .into_iter()
        .next()
        .expect("the body paragraph")
        .location()
        .clone();
    doc.set_story_text(&location, "c\r\nd").unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(
        texts(&doc),
        ["c\nd"],
        "as the paragraph text setter writes it"
    );
}

#[test]
fn paragraph_text_setter_keeps_a_lone_carriage_return_and_doubled_line_endings() {
    let mut doc = doc_with(&["old"]);
    doc.paragraph_mut(0)
        .unwrap()
        .set_text("a\rb\r\n\r\nc\n\rd")
        .unwrap();
    let doc = reopen(&mut doc);
    let paragraph = doc.paragraph(0).unwrap();
    assert_eq!(
        run_items(&paragraph, 0),
        [
            "text:a",
            "break:Line",
            "text:b",
            "break:Line",
            "break:Line",
            "text:c",
            "break:Line",
            "break:Line",
            "text:d"
        ],
        "a lone \\r is one break, two Windows endings two, a \\n then a \\r two"
    );
    assert_eq!(texts(&doc), ["a\nb\n\nc\n\nd"]);
}

#[test]
fn paragraph_text_shows_non_breaking_hyphens_and_positional_tabs() {
    let doc = doc_with_body(concat!(
        r#"<w:p><w:r><w:t>co</w:t><w:noBreakHyphen/><w:t>op</w:t>"#,
        r#"<w:ptab w:relativeTo="margin" w:alignment="right" w:leader="none"/>"#,
        r#"<w:t>page 2</w:t></w:r></w:p>"#,
    ));
    let text = doc.paragraph(0).unwrap().text();
    assert!(
        matches!(text.as_str(), "co-op\tpage 2" | "co\u{2011}op\tpage 2"),
        "a non-breaking hyphen and a positional tab read as text: {text:?}"
    );
}

#[test]
fn a_search_finds_and_replaces_text_across_a_non_breaking_hyphen_and_a_positional_tab() {
    let body = concat!(
        r#"<w:p><w:r><w:t>co</w:t><w:noBreakHyphen/><w:t>op</w:t>"#,
        r#"<w:ptab w:relativeTo="margin" w:alignment="right" w:leader="none"/>"#,
        r#"<w:t>page 2</w:t></w:r></w:p>"#,
    );
    let mut doc = doc_with_body(body);
    assert_eq!(
        doc.replace_text("coop", "X"),
        0,
        "the hyphen is part of the text"
    );
    assert_eq!(doc.replace_text("co-op", "cooperative"), 1);
    assert_eq!(doc.replace_text("\tpage", " page"), 1);
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["cooperative page 2"]);
    let xml = doc.part_xml("word/document.xml").unwrap();
    assert!(
        !xml.contains("noBreakHyphen") && !xml.contains("w:ptab"),
        "the matched hyphen and tab go: {xml}"
    );
    // The runs read them too, and so does a cell holding them.
    let doc = doc_with_body(body);
    let run_text: String = doc
        .paragraph(0)
        .unwrap()
        .runs()
        .map(|run| run.text())
        .collect();
    assert_eq!(run_text, "co-op\tpage 2");
    let doc = doc_with_body(&format!(
        r#"<w:tbl><w:tblGrid><w:gridCol w:w="4000"/></w:tblGrid><w:tr><w:tc>{body}</w:tc></w:tr></w:tbl>"#
    ));
    let table = doc.table(0).unwrap();
    assert_eq!(table.cell(0, 0).unwrap().text(), "co-op\tpage 2");
}

#[test]
fn counting_and_checked_replacement_see_non_breaking_hyphens_and_positional_tabs() {
    let mut doc =
        doc_with_body(r#"<w:p><w:r><w:t>co</w:t><w:noBreakHyphen/><w:t>op</w:t></w:r></w:p>"#);
    assert_eq!(doc.count_text("co-op").unwrap(), [(StoryKind::Body, 1)]);
    assert!(doc.count_text("coop").unwrap().is_empty());
    // "coop" is not in the document, so nothing is left out of reach.
    assert_eq!(
        doc.try_replace_all_checked(&[("coop", "X", None)], ReplaceOptions::default())
            .unwrap(),
        Ok(vec![0])
    );
    assert_eq!(
        doc.try_replace_all_checked(&[("co-op", "partner", None)], ReplaceOptions::default())
            .unwrap(),
        Ok(vec![1])
    );
    assert_eq!(texts(&doc), ["partner"]);
}

#[test]
fn a_footer_positional_tab_reads_as_a_tab_and_goes_with_a_new_text() {
    // Word's built-in footers put the page number after a right-aligned
    // positional tab.
    let mut doc = doc_with(&["body"]);
    doc.set_raw_footer_with_images(
        concat!(
            r#"<w:ftr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">"#,
            r#"<w:p><w:r><w:t>Confidential</w:t>"#,
            r#"<w:ptab w:relativeTo="margin" w:alignment="right" w:leader="none"/>"#,
            r#"<w:t>Page 1</w:t></w:r></w:p></w:ftr>"#,
        )
        .as_bytes()
        .to_vec(),
        &[],
        HdrFtrType::Default,
    );
    let footer = doc
        .stories()
        .unwrap()
        .into_iter()
        .find(|story| story.kind() == StoryKind::Footer)
        .expect("the footer is a story");
    let paragraph = doc
        .story_items(&footer)
        .unwrap()
        .into_iter()
        .find(|item| item.kind() == StoryItemKind::Paragraph)
        .expect("the footer has a paragraph");
    assert_eq!(
        paragraph.text().unwrap().as_deref(),
        Some("Confidential\tPage 1")
    );
    let location = paragraph.location().clone();
    assert_eq!(
        doc.count_text("Confidential\tPage").unwrap(),
        [(StoryKind::Footer, 1)]
    );
    doc.set_story_text(&location, "Draft").unwrap();
    let doc = reopen(&mut doc);
    let footer = doc
        .stories()
        .unwrap()
        .into_iter()
        .find(|story| story.kind() == StoryKind::Footer)
        .unwrap();
    let text = doc
        .story_items(&footer)
        .unwrap()
        .into_iter()
        .find(|item| item.kind() == StoryItemKind::Paragraph)
        .unwrap()
        .text()
        .unwrap();
    assert_eq!(text.as_deref(), Some("Draft"));
    let xml = doc
        .part_xml(footer.part_name().trim_start_matches('/'))
        .unwrap();
    assert!(
        !xml.contains("ptab"),
        "the tab goes with the old text: {xml}"
    );
}

#[test]
fn a_simple_field_result_keeps_its_non_breaking_hyphen() {
    let doc = doc_with_body(concat!(
        r#"<w:p><w:fldSimple w:instr=" DOCPROPERTY Title ">"#,
        r#"<w:r><w:rPr><w:b/></w:rPr><w:t>co</w:t><w:noBreakHyphen/><w:t>op</w:t></w:r>"#,
        r#"</w:fldSimple></w:p>"#,
    ));
    assert_eq!(texts(&doc), ["co-op"]);
    let paragraph = doc.paragraph(0).unwrap();
    let results: Vec<String> = paragraph
        .runs()
        .flat_map(|run| {
            run.items()
                .filter_map(|item| match item {
                    RunItemRef::Field(field) => Some(field.cached_result().to_owned()),
                    _ => None,
                })
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(results, ["co-op"], "the field result reads the hyphen too");
}

#[test]
fn an_updated_field_result_drops_the_old_non_breaking_hyphen() {
    for body in [
        concat!(
            r#"<w:p><w:fldSimple w:instr=" DOCPROPERTY Title ">"#,
            r#"<w:r><w:t>co</w:t><w:noBreakHyphen/><w:t>op</w:t></w:r>"#,
            r#"</w:fldSimple></w:p>"#,
        ),
        concat!(
            r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r>"#,
            r#"<w:r><w:instrText xml:space="preserve"> DOCPROPERTY Title </w:instrText></w:r>"#,
            r#"<w:r><w:fldChar w:fldCharType="separate"/></w:r>"#,
            r#"<w:r><w:t>co</w:t><w:noBreakHyphen/><w:t>op</w:t></w:r>"#,
            r#"<w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#,
        ),
    ] {
        let mut doc = doc_with_body(body);
        assert_eq!(texts(&doc), ["co-op"]);
        doc.set_title("Partner");
        doc.update_fields(&FieldEvaluationContext::default())
            .unwrap();
        let doc = reopen(&mut doc);
        assert_eq!(texts(&doc), ["Partner"], "{body}");
    }
}

#[test]
fn a_paragraph_holding_a_non_breaking_hyphen_and_a_positional_tab_lays_out() {
    // The layout counts each as one character of the run text.
    let doc = doc_with_body(concat!(
        r#"<w:p><w:r><w:t>co</w:t><w:noBreakHyphen/><w:t>op</w:t>"#,
        r#"<w:ptab w:relativeTo="margin" w:alignment="right" w:leader="none"/>"#,
        r#"<w:t>page 2</w:t></w:r></w:p>"#,
    ));
    let result = doc.layout().expect("the document lays out");
    assert_eq!(result.layout.pages.len(), 1);
}

#[test]
fn leading_trailing_and_repeated_spaces_survive_a_save() {
    let mut doc = Document::new();
    let mut paragraph = doc.add_paragraph("");
    paragraph.add_run("  two leading");
    paragraph.add_run(" ");
    paragraph.add_run("two  inside");
    paragraph.add_run("two trailing  ");
    doc.add_paragraph("   ");
    doc.add_paragraph("[").add_run("{{gap}}");
    doc.last_paragraph_mut().unwrap().add_run("]");
    assert_eq!(doc.replace_text("{{gap}}", "  spaced  "), 1);

    let doc = reopen(&mut doc);
    assert_eq!(
        run_texts(&doc, 0),
        ["  two leading", " ", "two  inside", "two trailing  "]
    );
    assert_eq!(
        texts(&doc),
        [
            "  two leading two  insidetwo trailing  ",
            "   ",
            "[  spaced  ]"
        ]
    );
    // The document text has one line per paragraph, blank ones included.
    assert!(
        doc.text().ends_with("two trailing  \n   \n[  spaced  ]\n"),
        "{:?}",
        doc.text()
    );
}

// ---- Unicode ----

#[test]
fn emoji_combining_marks_rtl_cjk_and_markup_characters_survive_a_save() {
    let samples = [
        "Emoji 👩‍💻 🇫🇷 👍🏽",
        "Combining e\u{301}te\u{301} and n\u{303}",
        "مرحبا بالعالم",
        "שלום עולם",
        "中文：你好，世界",
        "日本語のテキスト、カタカナ",
        "한국어 텍스트",
        "Ünïcödé — “quotes” … ¼",
        r#"Markup & entities: <b>not bold</b> "q" 'a' ]]> &amp;"#,
    ];
    let mut doc = doc_with(&samples);
    doc.add_paragraph("").add_run("混合 mixed עברית 🙂");

    let doc = reopen(&mut doc);
    let mut expected: Vec<String> = samples.iter().map(|s| (*s).to_owned()).collect();
    expected.push("混合 mixed עברית 🙂".to_owned());
    assert_eq!(texts(&doc), expected);
}

#[test]
fn right_to_left_and_east_asian_run_properties_survive_a_save() {
    let mut doc = Document::new();
    let mut paragraph = doc.add_paragraph("");
    let mut run = paragraph.add_run("مرحبا");
    run.set_rtl_value(Some(true));
    run.set_complex_script_value(Some(true));
    run.set_bold_cs_value(Some(true));
    run.set_size_cs_value(Some(16.0));
    run.set_language_bidi_value(Some("ar-SA"));
    run.set_slot_font(RunFontSlot::ComplexScript, Some("Arial"));
    let mut run = paragraph.add_run("東京 Tokyo");
    run.set_slot_font(RunFontSlot::Ascii, Some("Calibri"));
    run.set_slot_font(RunFontSlot::EastAsia, Some("MS Mincho"));
    run.set_language_east_asia_value(Some("ja-JP"));

    let doc = reopen(&mut doc);
    let paragraph = doc.paragraph(0).unwrap();
    let run = paragraph.run(0).unwrap();
    assert_eq!(run.text(), "مرحبا");
    assert_eq!(run.rtl_value(), Some(true));
    assert_eq!(run.complex_script_value(), Some(true));
    assert_eq!(run.bold_cs_value(), Some(true));
    assert_eq!(run.size_cs(), Some(16.0));
    assert_eq!(run.language_bidi(), Some("ar-SA"));
    assert_eq!(run.slot_font(RunFontSlot::ComplexScript), Some("Arial"));
    // The complex-script bold leaves the Latin bold alone.
    assert_eq!(run.bold_value(), None);

    let run = paragraph.run(1).unwrap();
    assert_eq!(run.text(), "東京 Tokyo");
    assert_eq!(run.slot_font(RunFontSlot::Ascii), Some("Calibri"));
    assert_eq!(run.slot_font(RunFontSlot::EastAsia), Some("MS Mincho"));
    assert_eq!(run.language_east_asia(), Some("ja-JP"));
}

// ---- Find and replace ----

#[test]
fn replace_text_replaces_every_occurrence_and_counts_them() {
    let mut doc = doc_with(&["a X b X", "c X", "Case case CASE"]);
    assert_eq!(doc.replace_text("X", "Y"), 3);
    assert_eq!(doc.replace_text("missing", "Z"), 0);
    assert_eq!(doc.replace_text("", "Z"), 0);
    // The search is case-sensitive.
    assert_eq!(doc.replace_text("case", "box"), 1);
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["a Y b Y", "c Y", "Case box CASE"]);
}

#[test]
fn a_replacement_containing_the_search_text_is_applied_once() {
    let mut doc = doc_with(&["Hello NAME!", "aaa"]);
    assert_eq!(doc.replace_text("NAME", "NAME Smith"), 1);
    // Each source "a" is replaced once: the output is not searched again.
    assert_eq!(doc.replace_text("a", "aa"), 3);
    assert_eq!(texts(&doc), ["Hello NAME Smith!", "aaaaaa"]);
}

#[test]
fn replacing_text_split_across_runs_keeps_the_first_run_format() {
    let mut doc = Document::new();
    let mut paragraph = doc.add_paragraph("");
    paragraph.add_run("Dear ");
    paragraph.add_run("{{na").bold(true);
    paragraph.add_run("me}}").italic(true);
    paragraph.add_run(", welcome.");

    assert_eq!(doc.replace_text("{{name}}", "Ada"), 1);
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["Dear Ada, welcome."]);
    let paragraph = doc.paragraph(0).unwrap();
    let replaced = paragraph
        .runs()
        .find(|run| run.text().contains("Ada"))
        .expect("a run holds the replacement");
    assert_eq!(replaced.text(), "Ada");
    assert_eq!(replaced.bold_value(), Some(true));
    assert_eq!(replaced.italic_value(), None);
}

#[test]
fn replacing_a_placeholder_word_split_with_rsids_and_proofing_marks() {
    // How Word saves "{{client_name}}" typed in several sittings and flagged
    // by the spell checker.
    let mut doc = doc_with_body(concat!(
        r#"<w:p w:rsidR="00A1B2C3" w:rsidRDefault="00D4E5F6">"#,
        r#"<w:r w:rsidRPr="00112233"><w:rPr><w:b/></w:rPr><w:t xml:space="preserve">Dear {{</w:t></w:r>"#,
        r#"<w:proofErr w:type="spellStart"/>"#,
        r#"<w:r w:rsidR="00445566"><w:rPr><w:b/></w:rPr><w:t>client</w:t></w:r>"#,
        r#"<w:r w:rsidR="00778899"><w:rPr><w:b/></w:rPr><w:t>_name</w:t></w:r>"#,
        r#"<w:proofErr w:type="spellEnd"/>"#,
        r#"<w:r w:rsidR="00445566"><w:rPr><w:b/></w:rPr><w:t>}},</w:t></w:r>"#,
        r#"</w:p>"#,
    ));
    assert_eq!(texts(&doc), ["Dear {{client_name}},"]);

    assert_eq!(doc.replace_text("{{client_name}}", "Ada Lovelace"), 1);
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["Dear Ada Lovelace,"]);
    let paragraph = doc.paragraph(0).unwrap();
    assert!(paragraph.runs().all(|run| run.bold_value() == Some(true)));
}

#[test]
fn replacing_text_split_inside_multibyte_characters_across_runs() {
    let mut doc = Document::new();
    let mut paragraph = doc.add_paragraph("");
    paragraph.add_run("你好");
    paragraph.add_run("世界 🙂");
    paragraph.add_run("🙂 end");

    assert_eq!(doc.replace_text("好世", "X"), 1);
    assert_eq!(doc.replace_text("🙂🙂", "😀"), 1);
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["你X界 😀 end"]);
}

#[test]
fn replace_all_replaces_every_placeholder_of_the_batch() {
    let mut doc = doc_with(&["Dear {{name}}, your order {{order}} ships {{date}}."]);
    let pairs = HashMap::from([
        ("{{name}}", "Ada"),
        ("{{order}}", "A-1"),
        ("{{date}}", "Friday"),
    ]);
    assert_eq!(doc.replace_all(&pairs), 3);
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["Dear Ada, your order A-1 ships Friday."]);
}

#[test]
fn replace_all_swaps_two_words_whatever_the_order_of_its_pairs() {
    // Each map has its own random order: sixteen of them make an order that
    // changes the result all but certain to show.
    for _ in 0..16 {
        let mut doc = doc_with(&["left right"]);
        let pairs = HashMap::from([("left", "right"), ("right", "left")]);
        let replaced = doc.replace_all(&pairs);
        let doc = reopen(&mut doc);
        assert_eq!(
            (replaced, texts(&doc)),
            (2, vec!["right left".to_owned()]),
            "the pairs of a batch apply at once"
        );
    }
}

#[test]
fn replace_all_never_searches_a_value_with_another_pair() {
    for _ in 0..8 {
        let mut doc = doc_with(&["alpha gamma"]);
        let pairs = HashMap::from([("alpha", "beta gamma"), ("gamma", "delta")]);
        assert_eq!(doc.replace_all(&pairs), 2);
        assert_eq!(texts(&reopen(&mut doc)), ["beta gamma delta"]);
    }
}

#[test]
fn replace_all_takes_the_longer_overlapping_placeholder_then_the_one_that_sorts_first() {
    for _ in 0..8 {
        let mut doc = doc_with(&["xyzw", "xyz", "abc€"]);
        assert_eq!(
            doc.replace_all(&HashMap::from([("xy", "1"), ("xyz", "2")])),
            2
        );
        assert_eq!(
            doc.replace_all(&HashMap::from([("yz", "4"), ("xy", "3")])),
            0
        );
        // Longer in characters, not in bytes: "abc" is longer than "c€".
        assert_eq!(
            doc.replace_all(&HashMap::from([("c€", "5"), ("abc", "6")])),
            1
        );
        assert_eq!(texts(&reopen(&mut doc)), ["2w", "2", "6€"]);
    }
    for _ in 0..8 {
        let mut doc = doc_with(&["xyz"]);
        assert_eq!(
            doc.replace_all(&HashMap::from([("yz", "2"), ("xy", "1")])),
            1
        );
        assert_eq!(texts(&reopen(&mut doc)), ["1z"]);
    }
}

#[test]
fn replace_all_writes_a_value_holding_private_use_characters_as_given() {
    // The marker the second pair would get without a delimiter of its own.
    let private = "\u{10FFFD}\u{100001}\u{10FFFD}";
    for _ in 0..8 {
        let mut doc = doc_with(&["alpha beta"]);
        let pairs = HashMap::from([("alpha", private), ("beta", "gamma")]);
        assert_eq!(doc.replace_all(&pairs), 2);
        assert_eq!(texts(&reopen(&mut doc)), [format!("{private} gamma")]);
    }
}

#[test]
fn replace_all_leaves_no_marker_in_a_corpus_document() {
    // The body and the header of documents saved by Word and Google Docs,
    // swapped and swapped back.
    for producer in ["word", "google"] {
        let mut doc = corpus_doc(&format!("{producer}/report.docx"));
        let before = doc.text_report().text;
        let swap = HashMap::from([("Northwind", "Operations"), ("Operations", "Northwind")]);
        let replaced = doc.replace_all(&swap);
        assert!(replaced >= 3, "{producer}: {replaced}");
        assert_eq!(doc.replace_all(&swap), replaced, "{producer}");
        let after = reopen(&mut doc).text_report().text;
        assert_eq!(after, before, "{producer}: swapped back");
        assert!(
            !after
                .chars()
                .any(|c| ('\u{100000}'..='\u{10FFFD}').contains(&c)),
            "{producer}: no private marker is left"
        );
    }
}

#[test]
fn counted_replacement_runs_pairs_in_order_and_reports_each_count() {
    let mut doc = doc_with(&["cat and dog"]);
    let counts = doc
        .try_replace_all_expected(&[("cat", "dog", Some(1)), ("dog", "wolf", Some(2))])
        .unwrap()
        .unwrap();
    // The second pair sees the dog the first pair wrote.
    assert_eq!(counts, [1, 2]);
    assert_eq!(texts(&doc), ["wolf and wolf"]);
    assert_eq!(
        doc.try_replace_all_expected(&[]).unwrap().unwrap(),
        Vec::<usize>::new()
    );
}

#[test]
fn counted_replacement_changes_nothing_when_a_count_is_wrong() {
    let mut doc = doc_with(&["{{a}} {{b}} {{b}}"]);
    let before = doc.to_bytes().unwrap();
    let mismatch = doc
        .try_replace_all_expected(&[("{{a}}", "A", Some(1)), ("{{b}}", "B", Some(1))])
        .unwrap()
        .unwrap_err();
    assert_eq!(
        mismatch,
        ReplacementCountMismatch {
            index: 1,
            placeholder: "{{b}}".to_owned(),
            expected: 1,
            found: 2,
        }
    );
    // The first pair matched, and is not applied either.
    assert_eq!(texts(&doc), ["{{a}} {{b}} {{b}}"]);
    assert_eq!(doc.to_bytes().unwrap(), before);
}

#[test]
fn replace_text_reaches_headers_footers_and_table_cells() {
    let mut doc = doc_with(&["Body {{x}}"]);
    doc.set_header("Header {{x}}");
    doc.set_footer("Footer {{x}}");
    doc.add_table(1, 2)
        .cell(0, 1)
        .unwrap()
        .set_text("Cell {{x}}");

    assert_eq!(doc.replace_text("{{x}}", "done"), 4);
    let doc = reopen(&mut doc);
    assert_eq!(doc.paragraph(0).unwrap().text(), "Body done");
    assert_eq!(doc.header_text().as_deref(), Some("Header done"));
    assert_eq!(doc.footer_text().as_deref(), Some("Footer done"));
    assert!(doc.text().contains("Cell done"), "{:?}", doc.text());
}

#[test]
fn regex_replacement_supports_capture_groups_and_refuses_a_bad_pattern() {
    let mut doc = doc_with(&["Due 2026-11-21 and 2027-01-05."]);
    assert_eq!(
        doc.replace_regex(r"(\d{4})-(\d{2})-(\d{2})", "$3/$2/$1")
            .unwrap(),
        2
    );
    assert_eq!(texts(&doc), ["Due 21/11/2026 and 05/01/2027."]);

    let before = doc.to_bytes().unwrap();
    let error = doc.replace_regex("(", "x").unwrap_err();
    assert!(error.to_string().contains("invalid regex"), "{error}");
    assert_eq!(doc.to_bytes().unwrap(), before);
}

#[test]
fn text_xml_cannot_carry_is_refused_with_its_code_point() {
    let mut doc = doc_with(&["value"]);
    let error = doc.try_replace_text("value", "bad\u{1}").unwrap_err();
    assert!(error.to_string().contains("U+0001"), "{error}");
    assert_eq!(texts(&doc), ["value"]);
    assert_eq!(doc.try_replace_text("value", "good").unwrap(), 1);
    assert_eq!(texts(&doc), ["good"]);

    // A run accepts such text, and the save refuses it rather than writing
    // a file Word cannot open.
    doc.paragraph_mut(0).unwrap().add_run("vertical\u{b}tab");
    let error = doc.to_bytes().unwrap_err();
    assert!(error.to_string().contains("U+000B"), "{error}");
}

#[test]
fn a_search_does_not_match_across_a_tab_inside_a_run() {
    let mut doc = doc_with(&[""]);
    doc.paragraph_mut(0).unwrap().set_text("Name\tAda").unwrap();
    // The paragraph reads "Name\tAda", so "eA" is not in it.
    let replaced = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        doc.replace_text("eA", "e A")
    }));
    assert!(
        matches!(replaced, Ok(0)),
        "a search does not match across a tab inside a run: {replaced:?}"
    );
    assert_eq!(texts(&doc), ["Name\tAda"]);
}

#[test]
fn searches_that_skipped_a_tab_inside_one_run_return_and_change_nothing() {
    // Before the search read tabs, these looped forever in release builds
    // (doubling the text) or panicked.
    for (body, search, replacement) in [
        (r#"<w:t>Name</w:t><w:tab/><w:t>Ada</w:t>"#, "eA", "e A"),
        (
            r#"<w:t xml:space="preserve">Name: </w:t><w:tab/><w:t xml:space="preserve"> John</w:t>"#,
            "  ",
            " ",
        ),
        (r#"<w:t>A</w:t><w:tab/><w:t>CME</w:t>"#, "ACME", "Acme"),
    ] {
        let mut doc = doc_with_body(&format!("<w:p><w:r>{body}</w:r></w:p>"));
        let before = texts(&doc);
        assert_eq!(doc.replace_text(search, replacement), 0, "{search:?}");
        assert_eq!(texts(&doc), before, "{search:?}");
    }
    // The same text with its tab in the search is replaced, tab included.
    let mut doc = doc_with_body(r#"<w:p><w:r><w:t>A</w:t><w:tab/><w:t>CME</w:t></w:r></w:p>"#);
    assert_eq!(doc.replace_text("A\tCME", "Acme"), 1);
    assert_eq!(texts(&reopen(&mut doc)), ["Acme"]);
}

#[test]
fn a_search_sees_tabs_and_breaks_as_paragraph_text_shows_them() {
    let mut doc = doc_with(&[""]);
    doc.paragraph_mut(0)
        .unwrap()
        .set_text("Total:\t42")
        .unwrap();
    assert_eq!(
        doc.replace_text("Total:\t42", "Total: 43"),
        1,
        "a search holding a tab finds it"
    );

    let mut doc = Document::new();
    let mut paragraph = doc.add_paragraph("");
    paragraph.add_run("Name");
    paragraph.add_line_break();
    paragraph.add_run("Ada");
    assert_eq!(texts(&doc), ["Name\nAda"]);
    // Today this matches "eA" across the break and gives "Name A\nda".
    assert_eq!(
        doc.replace_text("eA", "e A"),
        0,
        "a search does not match across a break"
    );
    assert_eq!(texts(&doc), ["Name\nAda"]);
}

#[test]
fn a_search_holding_a_tab_finds_it_however_the_producer_split_the_runs() {
    for (producer, body) in [
        (
            "Word: the tab in a run of its own",
            r#"<w:p><w:r><w:t>Total:</w:t></w:r><w:r><w:tab/></w:r><w:r><w:t>42</w:t></w:r></w:p>"#,
        ),
        (
            "LibreOffice: the tab between two texts of one run",
            r#"<w:p><w:r><w:t>Total:</w:t><w:tab/><w:t>42</w:t></w:r></w:p>"#,
        ),
    ] {
        let mut doc = doc_with_body(body);
        assert_eq!(texts(&doc), ["Total:\t42"], "{producer}");
        assert_eq!(doc.replace_text(":\t4", ":\t5"), 1, "{producer}");
        assert_eq!(texts(&doc), ["Total:\t52"], "{producer}");
        let xml = doc.part_xml("word/document.xml").unwrap();
        assert_eq!(xml.matches("<w:tab/>").count(), 1, "{producer}: {xml}");
        assert!(
            !xml.contains('\t'),
            "{producer}: no tab character in w:t: {xml}"
        );
        assert_eq!(doc.replace_text("\t52", " 52"), 1, "{producer}");
        assert_eq!(texts(&reopen(&mut doc)), ["Total: 52"], "{producer}");
    }
}

#[test]
fn a_match_across_runs_removes_every_matched_tab_and_text() {
    // The first run holds text, a tab and text: a match that starts in it
    // and ends in the next run takes all three.
    let mut doc = doc_with_body(
        r#"<w:p><w:r><w:t>ab</w:t><w:tab/><w:t>cd</w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:t>e</w:t><w:br/><w:t>fg</w:t></w:r></w:p>"#,
    );
    assert_eq!(texts(&doc), ["ab\tcde\nfg"]);
    assert_eq!(doc.replace_text("b\tcde\nf", "X"), 1);
    assert_eq!(texts(&reopen(&mut doc)), ["aXg"]);
}

#[test]
fn a_match_that_starts_on_a_tab_puts_the_replacement_in_its_place() {
    // The bold run of the tab takes the replacement, the italic run of the
    // number loses its text.
    let mut doc = doc_with_body(
        r#"<w:p><w:r><w:t>Total:</w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:tab/></w:r><w:r><w:rPr><w:i/></w:rPr><w:t>42</w:t></w:r></w:p>"#,
    );
    assert_eq!(doc.replace_text("\t42", " 43"), 1);
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["Total: 43"]);
    let xml = doc.part_xml("word/document.xml").unwrap();
    assert!(!xml.contains("<w:tab/>"), "the matched tab goes: {xml}");
    assert!(
        xml.contains(r#"<w:b/></w:rPr><w:t xml:space="preserve"> 43</w:t>"#),
        "the tab's run holds the replacement: {xml}"
    );
    assert!(!xml.contains("<w:i/>"), "the emptied run goes: {xml}");
}

#[test]
fn a_replacement_holding_a_break_or_a_tab_writes_them_as_items() {
    let mut doc =
        doc_with_body(r#"<w:p><w:r><w:t>Main St</w:t><w:br/><w:t>Paris</w:t></w:r></w:p>"#);
    assert_eq!(doc.replace_text("Main St\nParis", "Main St\nLyon"), 1);
    assert_eq!(doc.replace_text("Lyon", "Lyon\t69001"), 1);
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["Main St\nLyon\t69001"]);
    let xml = doc.part_xml("word/document.xml").unwrap();
    assert!(
        xml.contains("<w:t>Main St</w:t><w:br/><w:t>Lyon</w:t><w:tab/><w:t>69001</w:t>"),
        "a break and a tab as Word draws them: {xml}"
    );
}

#[test]
fn a_match_across_runs_keeps_the_unmodelled_children_of_its_first_and_last_runs() {
    let mut doc = doc_with_body(
        r#"<w:p><w:r><w:t>a</w:t><w:tab/><w:lastRenderedPageBreak/><w:t>b</w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:t>c</w:t><w:lastRenderedPageBreak/><w:t>d</w:t></w:r></w:p>"#,
    );
    assert_eq!(texts(&doc), ["a\tbcd"]);
    assert_eq!(doc.replace_text("\tbc", "X"), 1);
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["aXd"]);
    let xml = doc.part_xml("word/document.xml").unwrap();
    assert_eq!(
        xml.matches("<w:lastRenderedPageBreak/>").count(),
        2,
        "both marks stay: {xml}"
    );
    assert!(
        xml.contains("<w:t>a</w:t><w:t>X</w:t><w:lastRenderedPageBreak/></w:r>"),
        "the first run keeps its mark after the replacement: {xml}"
    );
    assert!(
        xml.contains("<w:lastRenderedPageBreak/><w:t>d</w:t>"),
        "the last run keeps its mark before its kept text: {xml}"
    );
}

#[test]
fn a_replacement_keeps_the_unmodelled_children_of_its_run() {
    let mut doc = doc_with_body(
        r#"<w:p><w:r><w:t>a</w:t><w:tab/><w:t>b</w:t><w:lastRenderedPageBreak/><w:t>c</w:t></w:r></w:p>"#,
    );
    assert_eq!(doc.replace_text("a\tb", "X"), 1);
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["Xc"]);
    let xml = doc.part_xml("word/document.xml").unwrap();
    assert!(
        xml.contains("<w:t>X</w:t><w:lastRenderedPageBreak/><w:t>c</w:t>"),
        "the page break mark keeps its place: {xml}"
    );
}

#[test]
fn a_replacement_with_windows_line_endings_resumes_the_search_where_it_ends() {
    // "\r\n" is written as one break, which reads back as one "\n": the
    // search goes on right after it, on a character boundary.
    let mut doc = doc_with_body(r#"<w:p><w:r><w:t>«{adresse}»</w:t></w:r></w:p>"#);
    assert_eq!(doc.replace_text("{adresse}", "1 rue Haute\r\nParis"), 1);
    assert_eq!(texts(&doc), ["«1 rue Haute\nParis»"]);

    let mut doc = doc_with_body(r#"<w:p><w:r><w:t>Ligne 1</w:t><w:br/><w:t>Été</w:t></w:r></w:p>"#);
    assert_eq!(doc.replace_text("\n", "\r\n"), 1);
    assert_eq!(texts(&doc), ["Ligne 1\nÉté"]);

    let mut doc = doc_with_body(r#"<w:p><w:r><w:t>{x}{x}</w:t></w:r></w:p>"#);
    assert_eq!(
        doc.replace_text("{x}", "\r\n"),
        2,
        "the next match is not skipped"
    );
    assert_eq!(texts(&reopen(&mut doc)), ["\n\n"]);
}

#[test]
fn a_template_value_with_a_tab_or_a_line_ending_renders_as_a_tab_or_a_break() {
    let mut doc = doc_with_body(r#"<w:p><w:r><w:t>{{ addr }}</w:t></w:r></w:p>"#);
    doc.render_template(&serde_json::json!({ "addr": "Main St\r\nParis\tFR" }))
        .unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["Main St\nParis\tFR"]);
    let xml = doc.part_xml("word/document.xml").unwrap();
    assert!(
        xml.contains("<w:t>Main St</w:t><w:br/><w:t>Paris</w:t><w:tab/><w:t>FR</w:t>"),
        "{xml}"
    );
}

#[test]
fn a_template_tag_after_a_tab_or_a_break_renders_in_place() {
    // Template tags read only the run text, so a tab or a break before a
    // tag must not shift the replacement.
    let mut doc = doc_with_body(
        r#"<w:p><w:r><w:t>Name:</w:t><w:tab/><w:t>{{ name }}</w:t></w:r></w:p><w:p><w:r><w:t>{{ street }}</w:t><w:br/><w:t>{{ city }}</w:t></w:r></w:p>"#,
    );
    doc.render_template(&serde_json::json!({
        "name": "Ada",
        "street": "Main St",
        "city": "Paris"
    }))
    .unwrap();
    assert_eq!(texts(&reopen(&mut doc)), ["Name:\tAda", "Main St\nParis"]);
}

#[test]
fn a_match_over_a_symbol_in_a_run_keeps_the_symbol() {
    // A symbol has no text, so a search reads over it; the replacement takes
    // the matched text and leaves the symbol after it.
    let mut doc = doc_with_body(
        r#"<w:p><w:r><w:t xml:space="preserve">before </w:t><w:sym w:font="Wingdings" w:char="F04A"/><w:t>x</w:t></w:r></w:p>"#,
    );
    assert_eq!(texts(&doc), ["before x"]);
    assert_eq!(doc.replace_text("before x", "after"), 1);
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["after"]);
    let xml = doc.part_xml("word/document.xml").unwrap();
    assert!(
        xml.contains(r#">after</w:t><w:sym w:font="Wingdings" w:char="F04A"/>"#),
        "the symbol stays after the replacement: {xml}"
    );
}

// ---- Producer files ----

#[test]
fn a_word_saved_report_reads_and_edits_its_bold_lead_in_run() {
    let mut doc = corpus_doc("word/report.docx");
    let index = texts(&doc)
        .iter()
        .position(|text| text.starts_with("Summary. "))
        .expect("the summary paragraph");
    let paragraph = doc.paragraph(index).unwrap();
    let lead_in = paragraph.run(0).unwrap();
    assert_eq!(lead_in.text(), "Summary. ");
    assert!(lead_in.is_bold());
    assert!(!paragraph.run(1).unwrap().is_bold());

    doc.paragraph_mut(index)
        .unwrap()
        .run_mut(0)
        .unwrap()
        .set_text("Overview. ");
    let doc = reopen(&mut doc);
    let paragraph = doc.paragraph(index).unwrap();
    assert!(paragraph.text().starts_with("Overview. Revenue grew"));
    assert!(paragraph.run(0).unwrap().is_bold());
}

#[test]
fn replacing_across_formatted_runs_of_a_google_saved_report() {
    let mut doc = corpus_doc("google/report.docx");
    let find = |doc: &Document| {
        texts(doc)
            .iter()
            .position(|text| text.starts_with("Median first response time"))
            .expect("the support paragraph")
    };
    let index = find(&doc);
    let paragraph = doc.paragraph(index).unwrap();
    // Google writes `w:b w:val="1"` and an explicit `w:rtl w:val="0"`.
    assert_eq!(paragraph.run(1).unwrap().text(), "6 h 40");
    assert_eq!(paragraph.run(1).unwrap().bold_value(), Some(true));
    assert_eq!(paragraph.run(0).unwrap().rtl_value(), Some(false));

    // "40 to 3" crosses a bold, a plain and a bold run.
    assert_eq!(doc.replace_text("40 to 3", "40 down to 3"), 1);
    let doc = reopen(&mut doc);
    let index = find(&doc);
    let paragraph = doc.paragraph(index).unwrap();
    assert!(
        paragraph
            .text()
            .starts_with("Median first response time went from 6 h 40 down to 3 h 15."),
        "{}",
        paragraph.text()
    );
    let bold: String = paragraph
        .runs()
        .filter(|run| run.bold_value() == Some(true))
        .map(|run| run.text())
        .collect();
    assert!(bold.starts_with("6 h 40"), "{bold:?}");
}

#[test]
fn replacing_text_in_every_corpus_report_survives_a_save() {
    for name in [
        "word/report.docx",
        "google/report.docx",
        "libreoffice/report.docx",
        "authored/report.docx",
    ] {
        let mut doc = corpus_doc(name);
        let found = doc.text().matches("Lyon").count();
        assert!(found > 0, "{name}: no Lyon in the body");
        let replaced = doc.replace_text("Lyon", "Lille");
        assert_eq!(replaced, found, "{name}");
        let doc = reopen(&mut doc);
        assert!(!doc.text().contains("Lyon"), "{name}");
        assert_eq!(doc.text().matches("Lille").count(), found, "{name}");
    }
}

// ---- Hyperlinks as text ----

#[test]
fn a_hyperlink_run_reads_back_its_text_and_url() {
    let mut doc = doc_with(&["See "]);
    doc.append_hyperlink("the docs", "https://example.com/docs?a=1&b=2");
    doc.last_paragraph_mut().unwrap().add_run(" for more.");

    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["See the docs for more."]);
    let links = doc.links();
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].text, "the docs");
    assert_eq!(
        links[0].url.as_deref(),
        Some("https://example.com/docs?a=1&b=2")
    );
    let paragraph = doc.paragraph(0).unwrap();
    let link = paragraph
        .items()
        .find_map(|item| match item {
            ParagraphItemRef::Hyperlink(link) => Some(link),
            _ => None,
        })
        .expect("a hyperlink item");
    assert_eq!(link.text(), "the docs");
    let rel_id = link.relationship_id().expect("an external link");
    assert_eq!(
        doc.hyperlink_url(rel_id).as_deref(),
        Some("https://example.com/docs?a=1&b=2")
    );
}

#[test]
fn hyperlink_text_takes_run_formatting_and_the_hyperlink_style() {
    let mut doc = Document::new();
    let rel_id = doc.add_hyperlink_relationship("https://example.com/");
    let mut paragraph = doc.add_paragraph("Visit ");
    paragraph.add_hyperlink("our site", &rel_id).bold(true);

    let doc = reopen(&mut doc);
    let paragraph = doc.paragraph(0).unwrap();
    let link_run = paragraph.run(1).unwrap();
    assert_eq!(link_run.text(), "our site");
    assert_eq!(link_run.bold_value(), Some(true));
    assert_eq!(link_run.style_id(), Some("Hyperlink"));
    assert_eq!(doc.links()[0].url.as_deref(), Some("https://example.com/"));
}

#[test]
fn editing_the_text_of_a_hyperlink_keeps_its_target() {
    let mut doc = doc_with(&["Read "]);
    doc.append_hyperlink("this", "https://example.com/a");
    let mut doc = reopen(&mut doc);

    doc.paragraph_mut(0)
        .unwrap()
        .run_mut(1)
        .unwrap()
        .set_text("the full guide");
    assert_eq!(doc.replace_text("full", "short"), 1);

    let doc = reopen(&mut doc);
    let links = doc.links();
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].text, "the short guide");
    assert_eq!(links[0].url.as_deref(), Some("https://example.com/a"));
}

#[test]
fn splitting_a_hyperlink_run_keeps_both_halves_in_the_link() {
    let mut doc = Document::new();
    doc.append_hyperlink("click here", "https://example.com/");
    doc.split_run(0, 0, 5).unwrap();

    let doc = reopen(&mut doc);
    assert_eq!(run_texts(&doc, 0), ["click", " here"]);
    let links = doc.links();
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].text, "click here");
}

#[test]
fn removing_a_hyperlink_keeps_its_text_as_plain_text() {
    let mut doc = doc_with(&["Go "]);
    doc.append_hyperlink("home", "https://example.com/");
    let body = doc
        .stories()
        .unwrap()
        .into_iter()
        .find(|story| story.kind() == StoryKind::Body)
        .expect("a body story");
    doc.remove_hyperlink(&body, 0).unwrap();

    let doc = reopen(&mut doc);
    assert!(doc.links().is_empty());
    assert_eq!(texts(&doc), ["Go home"]);
    // Word's Remove Hyperlink also drops the Hyperlink character style.
    assert_eq!(doc.paragraph(0).unwrap().run(1).unwrap().style_id(), None);
}

#[test]
fn replace_all_rewrites_a_placeholder_found_only_in_a_text_box_fallback() {
    // Word writes a text box twice; replacement rewrites the VML copy in
    // mc:Fallback without counting it, so nothing is counted here.
    let body = r#"<w:p><w:r><mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:v="urn:schemas-microsoft-com:vml"><mc:Choice Requires="wps"><w:drawing><wp:inline distT="0" distB="0" distL="0" distR="0"><wp:extent cx="914400" cy="914400"/><wp:docPr id="80" name="Text Box 80"/><a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:graphicData uri="http://schemas.microsoft.com/office/word/2010/wordprocessingShape"><wps:wsp><wps:cNvSpPr txBox="1"/><wps:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="914400" cy="914400"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></wps:spPr><wps:txbx><w:txbxContent><w:p><w:r><w:t>plain box</w:t></w:r></w:p></w:txbxContent></wps:txbx><wps:bodyPr/></wps:wsp></a:graphicData></a:graphic></wp:inline></w:drawing></mc:Choice><mc:Fallback><w:pict><v:shape id="tb80"><v:textbox><w:txbxContent><w:p><w:r><w:t>{name} box</w:t></w:r></w:p></w:txbxContent></v:textbox></v:shape></w:pict></mc:Fallback></mc:AlternateContent></w:r></w:p>"#;
    let mut doc = doc_with_body(body);
    let replaced = doc.replace_all(&HashMap::from([("{name}", "Bob"), ("other", "x")]));
    assert_eq!(replaced, 0, "the fallback copy is not counted");
    let bytes = doc.to_bytes().unwrap();
    let package = oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(bytes)).unwrap();
    let main = String::from_utf8(package.get_part("/word/document.xml").unwrap().to_vec()).unwrap();
    assert!(main.contains("Bob box"), "the fallback copy is rewritten");
    assert!(
        !main
            .chars()
            .any(|c| ('\u{100000}'..='\u{10FFFD}').contains(&c)),
        "no private marker is left"
    );
}

// ---- Find and isolate a span ----

/// The paragraph of the corpus report of `producer` that starts with `prefix`,
/// by its index in `Document::paragraphs`.
fn paragraph_starting(doc: &Document, prefix: &str) -> usize {
    doc.paragraphs()
        .iter()
        .position(|paragraph| paragraph.text().starts_with(prefix))
        .unwrap_or_else(|| panic!("no paragraph starting with {prefix:?}"))
}

/// Whether each character of the literal run text of paragraph `index` is
/// bold by direct formatting.
fn bold_characters(doc: &Document, index: usize) -> Vec<bool> {
    let paragraph = doc.paragraph(index).unwrap();
    (0..paragraph.run_count())
        .flat_map(|run| {
            let run = paragraph.run(run).unwrap();
            let bold = run.bold_value() == Some(true);
            run.text().chars().map(move |_| bold).collect::<Vec<_>>()
        })
        .collect()
}

fn runs_text(doc: &Document, index: usize, runs: std::ops::Range<usize>) -> String {
    let paragraph = doc.paragraph(index).unwrap();
    runs.map(|run| paragraph.run(run).unwrap().text()).collect()
}

#[test]
fn find_text_gives_the_span_of_the_chosen_match_in_the_paragraph() {
    for producer in ["word", "google"] {
        let doc = corpus_doc(&format!("{producer}/report.docx"));
        let index = paragraph_starting(&doc, "Summary. Revenue grew");
        let paragraph = doc.paragraph(index).unwrap();
        let text = paragraph.text();
        let span = paragraph.find_text("the", 0).expect("a first match");
        let chars = text.chars().collect::<Vec<_>>();
        assert_eq!(
            chars[span.start..span.end].iter().collect::<String>(),
            "the",
            "{producer}"
        );
        let second = paragraph.find_text("the", 1).expect("a second match");
        assert!(
            second.start >= span.end,
            "{producer}: matches do not overlap"
        );
        assert_eq!(paragraph.find_text("not in the paragraph", 0), None);
        assert_eq!(paragraph.find_text("", 0), None, "{producer}");
        assert_eq!(paragraph.find_text("the", 99), None, "{producer}");
    }
}

#[test]
fn isolate_span_makes_the_span_whole_runs_that_can_be_formatted() {
    for producer in ["word", "google"] {
        let mut doc = corpus_doc(&format!("{producer}/report.docx"));
        let index = paragraph_starting(&doc, "Summary. Revenue grew");
        let before = doc.paragraph(index).unwrap().text();
        let was_bold = bold_characters(&doc, index);
        let span = doc
            .paragraph(index)
            .unwrap()
            .find_text("grew by 12 %", 0)
            .unwrap();
        let runs = doc
            .paragraph_mut(index)
            .unwrap()
            .isolate_span(span)
            .unwrap();
        assert_eq!(
            runs_text(&doc, index, runs.clone()),
            "grew by 12 %",
            "{producer}"
        );
        assert_eq!(doc.paragraph(index).unwrap().text(), before, "{producer}");
        for run in runs.clone() {
            doc.paragraph_mut(index)
                .unwrap()
                .run_mut(run)
                .unwrap()
                .set_bold(true);
        }
        let doc = reopen(&mut doc);
        let paragraph = doc.paragraph(index).unwrap();
        assert_eq!(paragraph.text(), before, "{producer}");
        let expected = was_bold
            .iter()
            .enumerate()
            .map(|(at, bold)| *bold || (span.start..span.end).contains(&at))
            .collect::<Vec<_>>();
        assert_eq!(
            bold_characters(&doc, index),
            expected,
            "{producer}: the span is bold, the rest as it was"
        );
    }
}

#[test]
fn isolate_span_splits_runs_inside_an_insertion_and_an_inline_control() {
    let body = r#"<w:p><w:r><w:t xml:space="preserve">say AC</w:t></w:r><w:ins w:id="1" w:author="a" w:date="2026-01-01T00:00:00Z"><w:r><w:t xml:space="preserve">ME now</w:t></w:r></w:ins></w:p><w:p><w:r><w:t xml:space="preserve">say AC</w:t></w:r><w:sdt><w:sdtContent><w:r><w:t xml:space="preserve">ME now</w:t></w:r></w:sdtContent></w:sdt></w:p>"#;
    for index in [0, 1] {
        let mut doc = doc_with_body(body);
        let before = doc.paragraph(index).unwrap().text();
        let span = doc.paragraph(index).unwrap().find_text("ACME", 0).unwrap();
        assert_eq!((span.start, span.end), (4, 8));
        let runs = doc
            .paragraph_mut(index)
            .unwrap()
            .isolate_span(span)
            .unwrap();
        assert_eq!(
            runs.len(),
            2,
            "paragraph {index}: one run each side of the edge"
        );
        assert_eq!(runs_text(&doc, index, runs), "ACME", "paragraph {index}");
        let doc = reopen(&mut doc);
        assert_eq!(
            doc.paragraph(index).unwrap().text(),
            before,
            "paragraph {index}"
        );
    }
}

#[test]
fn find_text_does_not_search_a_simple_field_result() {
    let body = r#"<w:p><w:r><w:t xml:space="preserve">by </w:t></w:r><w:fldSimple w:instr=" AUTHOR "><w:r><w:t>Ann</w:t></w:r></w:fldSimple></w:p>"#;
    let doc = doc_with_body(body);
    let paragraph = doc.paragraph(0).unwrap();
    assert_eq!(
        paragraph.text(),
        "by Ann",
        "Paragraph::text shows the result"
    );
    assert_eq!(paragraph.find_text("Ann", 0), None);
}

#[test]
fn isolating_an_empty_or_outside_span_is_refused_and_changes_nothing() {
    let mut doc = doc_with(&["alpha beta"]);
    let before = doc.to_bytes().unwrap();
    for (start, end) in [(3, 3), (5, 40), (40, 41)] {
        assert!(
            doc.paragraph_mut(0)
                .unwrap()
                .isolate_span(oxoffice_docx::TextSpan { start, end })
                .is_err(),
            "{start}..{end}"
        );
    }
    assert_eq!(doc.to_bytes().unwrap(), before);
}

#[test]
fn find_text_never_crosses_a_tab_or_a_complex_field_result() {
    let body = r#"<w:p><w:r><w:t>Total:</w:t><w:tab/><w:t>100</w:t></w:r></w:p><w:p><w:r><w:t xml:space="preserve">Page </w:t></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> PAGE </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>1</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:t xml:space="preserve"> end</w:t></w:r></w:p>"#;
    let doc = doc_with_body(body);
    let total = doc.paragraph(0).unwrap();
    assert_eq!(total.find_text("Total:100", 0), None, "across the tab");
    assert_eq!(
        total.find_text("100", 0).map(|span| (span.start, span.end)),
        Some((6, 9))
    );
    let page = doc.paragraph(1).unwrap();
    assert_eq!(
        page.text(),
        "Page 1 end",
        "Paragraph::text shows the result"
    );
    assert_eq!(
        page.find_text("Page 1", 0),
        None,
        "the result is not searched"
    );
    assert_eq!(page.find_text("Page  end", 0), None, "nor crossed");
    assert!(page.find_text("Page", 0).is_some());
}

#[test]
fn isolate_span_across_runs_of_different_formatting_keeps_each_one() {
    for producer in ["word", "google"] {
        let mut doc = corpus_doc(&format!("{producer}/report.docx"));
        let index = paragraph_starting(&doc, "Summary. Revenue grew");
        let before = doc.paragraph(index).unwrap().text();
        let was_bold = bold_characters(&doc, index);
        assert_ne!(
            was_bold[0], was_bold[10],
            "{producer}: the span starts bold and ends plain"
        );
        let span = doc
            .paragraph(index)
            .unwrap()
            .find_text("Summary. Revenue", 0)
            .unwrap();
        let runs = doc
            .paragraph_mut(index)
            .unwrap()
            .isolate_span(span)
            .unwrap();
        assert!(runs.len() >= 2, "{producer}: {runs:?}");
        assert_eq!(
            runs_text(&doc, index, runs),
            "Summary. Revenue",
            "{producer}"
        );
        let doc = reopen(&mut doc);
        assert_eq!(doc.paragraph(index).unwrap().text(), before, "{producer}");
        assert_eq!(bold_characters(&doc, index), was_bold, "{producer}");
    }
}

#[test]
fn isolate_span_keeps_the_insertion_and_the_control_around_their_runs() {
    let body = r#"<w:p><w:r><w:t xml:space="preserve">say AC</w:t></w:r><w:ins w:id="1" w:author="a" w:date="2026-01-01T00:00:00Z"><w:r><w:t xml:space="preserve">ME now</w:t></w:r></w:ins></w:p><w:p><w:r><w:t xml:space="preserve">say AC</w:t></w:r><w:sdt><w:sdtPr><w:tag w:val="probe"/></w:sdtPr><w:sdtContent><w:r><w:t xml:space="preserve">ME now</w:t></w:r></w:sdtContent></w:sdt></w:p>"#;
    let mut doc = doc_with_body(body);
    for index in [0, 1] {
        let span = doc.paragraph(index).unwrap().find_text("ACME", 0).unwrap();
        doc.paragraph_mut(index)
            .unwrap()
            .isolate_span(span)
            .unwrap();
    }
    let doc = reopen(&mut doc);
    let insertions = doc
        .story_revisions()
        .unwrap()
        .iter()
        .map(|revision| revision.text().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(insertions, ["ME now"], "one insertion, all its text");
    let controls = doc
        .content_controls()
        .iter()
        .map(|control| control.text())
        .collect::<Vec<_>>();
    assert_eq!(controls, ["ME now"], "one control, all its text");
}

#[test]
fn isolate_span_leaves_a_tab_before_and_a_break_after_the_span_outside_its_runs() {
    let body = r#"<w:p><w:r><w:t>Name:</w:t><w:tab/><w:t>John Smith</w:t><w:br/><w:t>signed</w:t></w:r></w:p>"#;
    let mut doc = doc_with_body(body);
    let span = doc
        .paragraph(0)
        .unwrap()
        .find_text("John Smith", 0)
        .unwrap();
    assert_eq!((span.start, span.end), (5, 15), "the tab has no width");
    let runs = doc.paragraph_mut(0).unwrap().isolate_span(span).unwrap();
    assert_eq!(runs_text(&doc, 0, runs.clone()), "John Smith");
    for run in runs.clone() {
        doc.paragraph_mut(0)
            .unwrap()
            .run_mut(run)
            .unwrap()
            .set_bold(true);
    }
    let mut doc = reopen(&mut doc);
    let formatted = doc
        .paragraph(0)
        .unwrap()
        .runs()
        .map(|run| (run.text(), run.bold_value() == Some(true)))
        .collect::<Vec<_>>();
    assert_eq!(
        formatted,
        [
            ("Name:\t".to_owned(), false),
            ("John Smith".to_owned(), true),
            ("\nsigned".to_owned(), false),
        ],
        "the bold leaves the tab and the break alone"
    );
    doc.paragraph_mut(0)
        .unwrap()
        .run_mut(runs.start)
        .unwrap()
        .set_text("Jane Doe");
    assert_eq!(
        doc.paragraph(0).unwrap().text(),
        "Name:\tJane Doe\nsigned",
        "new text in the span keeps the tab and the break"
    );
}

#[test]
fn a_match_refused_at_a_tab_does_not_hide_the_next_one() {
    let doc = doc_with_body(r#"<w:p><w:r><w:t>a</w:t><w:tab/><w:t>aa</w:t></w:r></w:p>"#);
    let paragraph = doc.paragraph(0).unwrap();
    assert_eq!(
        paragraph
            .find_text("aa", 0)
            .map(|span| (span.start, span.end)),
        Some((1, 3))
    );
    assert_eq!(paragraph.find_text("aa", 1), None);
}

#[test]
fn find_text_never_crosses_a_picture_or_an_object_kept_as_raw_xml() {
    let body = r#"<w:p><w:r><w:t>left</w:t><w:pict><v:rect xmlns:v="urn:schemas-microsoft-com:vml" style="width:10pt;height:10pt"/></w:pict><w:t>right</w:t></w:r></w:p><w:p><w:r><w:t>Total</w:t><w:lastRenderedPageBreak/><w:t xml:space="preserve"> due</w:t></w:r></w:p>"#;
    let doc = doc_with_body(body);
    let picture = doc.paragraph(0).unwrap();
    assert_eq!(
        picture.find_text("leftright", 0),
        None,
        "across the picture"
    );
    assert_eq!(
        picture
            .find_text("right", 0)
            .map(|span| (span.start, span.end)),
        Some((4, 9))
    );
    assert!(
        doc.paragraph(1)
            .unwrap()
            .find_text("Total due", 0)
            .is_some(),
        "a last rendered page break shows nothing"
    );
}

#[test]
fn isolate_span_leaves_a_tab_starting_its_run_and_a_break_ending_it_outside() {
    // Word often starts a run with the tab that comes before its text.
    let body = r#"<w:p><w:r><w:t>Name:</w:t></w:r><w:r><w:rPr><w:i/></w:rPr><w:tab/><w:t>John Smith</w:t><w:br/></w:r><w:r><w:t>signed</w:t></w:r></w:p>"#;
    let mut doc = doc_with_body(body);
    let span = doc
        .paragraph(0)
        .unwrap()
        .find_text("John Smith", 0)
        .unwrap();
    let runs = doc.paragraph_mut(0).unwrap().isolate_span(span).unwrap();
    assert_eq!(runs_text(&doc, 0, runs.clone()), "John Smith");
    doc.paragraph_mut(0)
        .unwrap()
        .run_mut(runs.start)
        .unwrap()
        .set_text("Jane Doe");
    let doc = reopen(&mut doc);
    let paragraph = doc.paragraph(0).unwrap();
    assert_eq!(paragraph.text(), "Name:\tJane Doe\nsigned");
    let formatted = paragraph
        .runs()
        .map(|run| (run.text(), run.italic_value() == Some(true)))
        .collect::<Vec<_>>();
    assert_eq!(
        formatted,
        [
            ("Name:".to_owned(), false),
            ("\t".to_owned(), true),
            ("Jane Doe".to_owned(), true),
            ("\n".to_owned(), true),
            ("signed".to_owned(), false),
        ],
        "each piece of the italic run keeps its formatting"
    );
}

#[test]
fn isolate_span_does_not_split_off_what_shows_nothing() {
    let body = r#"<w:p><w:r><w:lastRenderedPageBreak/><w:t>John</w:t></w:r><w:r><w:t xml:space="preserve"> and </w:t></w:r><w:r><w:pict><v:rect xmlns:v="urn:schemas-microsoft-com:vml" style="width:10pt;height:10pt"/></w:pict><w:t>Ann</w:t></w:r></w:p>"#;
    let mut doc = doc_with_body(body);
    let john = doc.paragraph(0).unwrap().find_text("John", 0).unwrap();
    let runs = doc.paragraph_mut(0).unwrap().isolate_span(john).unwrap();
    assert_eq!(runs, 0..1, "the page break mark stays in the run");
    let ann = doc.paragraph(0).unwrap().find_text("Ann", 0).unwrap();
    let runs = doc.paragraph_mut(0).unwrap().isolate_span(ann).unwrap();
    assert_eq!(runs, 3..4, "the picture goes to a run of its own");
    assert_eq!(runs_text(&doc, 0, runs), "Ann");
    let doc = reopen(&mut doc);
    assert_eq!(doc.paragraph(0).unwrap().run_count(), 4);
    assert_eq!(doc.paragraph(0).unwrap().text(), "John and Ann");
}

/// The saved XML of the body run whose text element reads exactly `text`.
fn saved_run_holding(doc: &mut Document, text: &str) -> String {
    let bytes = doc.to_bytes().unwrap();
    let package = oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(bytes)).unwrap();
    let main = String::from_utf8(package.get_part("/word/document.xml").unwrap().to_vec()).unwrap();
    let at = main
        .find(&format!(">{text}</w:t>"))
        .unwrap_or_else(|| panic!("no text element {text:?}"));
    let start = main[..at]
        .rfind("<w:r>")
        .or(main[..at].rfind("<w:r "))
        .unwrap();
    let end = at + main[at..].find("</w:r>").unwrap();
    main[start..end].to_owned()
}

#[test]
fn isolate_span_leaves_a_picture_between_texts_or_ending_its_run_outside() {
    let pict = r#"<w:pict><v:rect xmlns:v="urn:schemas-microsoft-com:vml" style="width:10pt;height:10pt"/></w:pict>"#;
    let body = format!(
        "<w:p><w:r><w:t>left</w:t>{pict}<w:t>right</w:t></w:r></w:p>\
         <w:p><w:r><w:t>Ann</w:t>{pict}</w:r></w:p>\
         <w:p><w:r><w:rPr><w:i/></w:rPr><w:t>Name:</w:t><w:tab/>{pict}<w:t>John</w:t></w:r></w:p>"
    );
    let mut doc = doc_with_body(&body);
    for (index, word, runs_after) in [(0, "right", 2), (1, "Ann", 2), (2, "John", 2)] {
        let span = doc.paragraph(index).unwrap().find_text(word, 0).unwrap();
        let runs = doc
            .paragraph_mut(index)
            .unwrap()
            .isolate_span(span)
            .unwrap();
        assert_eq!(runs.len(), 1, "{word}");
        assert_eq!(runs_text(&doc, index, runs), word, "{word}");
        assert_eq!(
            doc.paragraph(index).unwrap().run_count(),
            runs_after,
            "{word}: the picture is in the other run"
        );
        assert!(
            !saved_run_holding(&mut doc, word).contains("w:pict"),
            "{word}: the picture is outside the span's run"
        );
    }
    // Each picture is kept once, where it was among the texts.
    let bytes = doc.to_bytes().unwrap();
    let package = oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(bytes)).unwrap();
    let main = String::from_utf8(package.get_part("/word/document.xml").unwrap().to_vec()).unwrap();
    let order = [
        "left", "w:pict", "right", "Ann", "w:pict", "Name:", "w:tab", "w:pict", "John",
    ];
    let mut from = 0;
    for mark in order {
        let at = main[from..]
            .find(mark)
            .unwrap_or_else(|| panic!("{mark} after byte {from}"));
        from += at + mark.len();
    }
    assert_eq!(main.matches("<w:pict>").count(), 3);
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["leftright", "Ann", "Name:\tJohn"]);
    assert_eq!(
        doc.paragraph(2).unwrap().run(1).unwrap().italic_value(),
        Some(true),
        "the span's run keeps the run properties"
    );
}

#[test]
fn isolate_span_at_the_edges_of_a_run_inside_an_insertion_or_a_control() {
    let run = r#"<w:r><w:tab/><w:t>John</w:t><w:br/></w:r>"#;
    let body = format!(
        r#"<w:p><w:ins w:id="1" w:author="a" w:date="2026-01-01T00:00:00Z">{run}</w:ins></w:p><w:p><w:sdt><w:sdtPr><w:tag w:val="probe"/></w:sdtPr><w:sdtContent>{run}</w:sdtContent></w:sdt></w:p>"#
    );
    let mut doc = doc_with_body(&body);
    for index in [0, 1] {
        let span = doc.paragraph(index).unwrap().find_text("John", 0).unwrap();
        let runs = doc
            .paragraph_mut(index)
            .unwrap()
            .isolate_span(span)
            .unwrap();
        assert_eq!(runs, 1..2, "paragraph {index}: tab, span, break");
        assert_eq!(runs_text(&doc, index, runs), "John", "paragraph {index}");
        assert_eq!(doc.paragraph(index).unwrap().run_count(), 3);
    }
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["\tJohn\n", "\tJohn\n"]);
    assert_eq!(doc.story_revisions().unwrap().len(), 1, "one insertion");
    assert_eq!(doc.content_controls().len(), 1, "one control");
}
