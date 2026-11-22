//! Runs and their text: run formatting, adding, editing, splitting and
//! removing runs, breaks and whitespace, unicode text, find and replace, and
//! hyperlinks read and edited as text.

use std::collections::HashMap;

use oxoffice_docx::{
    Document, FieldEvaluationContext, HdrFtrType, ParagraphItemRef, ParagraphRef, ReplaceOptions,
    ReplacementCountMismatch, RunFontSlot, RunItemRef, StoryItemKind, StoryKind, UnderlineStyle,
};

use crate::support::{doc_with, doc_with_body, reopen, texts};

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

// ---- Adding, editing, splitting and removing runs ----

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

// ---- Unicode ----

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

    // A run cannot refuse: it drops the character, and the document saves.
    doc.paragraph_mut(0).unwrap().add_run("vertical\u{b}tab");
    assert_eq!(texts(&reopen(&mut doc)), ["goodverticaltab"]);
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

/// A body with `<w:br {attributes}/>` in a run, in the result of a simple
/// field and in the result of a complex field, after a paragraph to edit.
fn doc_with_breaks(attributes: &str) -> Document {
    doc_with_body(&format!(
        concat!(
            r#"<w:p><w:r><w:t>Edit me</w:t></w:r></w:p>"#,
            r#"<w:p><w:r><w:t>before</w:t><w:br {a}/><w:t>after</w:t></w:r></w:p>"#,
            r#"<w:p><w:fldSimple w:instr=" DOCPROPERTY Title ">"#,
            r#"<w:r><w:t>simple</w:t><w:br {a}/><w:t>result</w:t></w:r>"#,
            r#"</w:fldSimple></w:p>"#,
            r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r>"#,
            r#"<w:r><w:instrText xml:space="preserve"> DOCPROPERTY Title </w:instrText></w:r>"#,
            r#"<w:r><w:fldChar w:fldCharType="separate"/></w:r>"#,
            r#"<w:r><w:t>complex</w:t><w:br {a}/><w:t>result</w:t></w:r>"#,
            r#"<w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#,
        ),
        a = attributes
    ))
}

/// The kind and clear of every `w:br` of `xml`, defaults filled in: a break
/// without a type is a line break, one without a clear clears nothing.
fn break_kinds_and_clears(xml: &str) -> Vec<(String, String)> {
    xml.match_indices("<w:br")
        .map(|(start, _)| {
            let tag = &xml[start..start + xml[start..].find('>').unwrap()];
            let attribute = |name: &str, default: &str| {
                tag.find(&format!(" w:{name}=\""))
                    .map(|at| {
                        let value = &tag[at + name.len() + 5..];
                        value[..value.find('"').unwrap()].to_owned()
                    })
                    .unwrap_or_else(|| default.to_owned())
            };
            (
                attribute("type", "textWrapping"),
                attribute("clear", "none"),
            )
        })
        .collect()
}

#[test]
fn a_break_keeps_where_its_text_restarts_through_an_edit_and_a_reopen() {
    // `w:clear` says where the text after the break restarts beside a
    // float. A page or column break keeps it too: the file had it.
    for (attributes, kind, clear) in [
        (
            r#"w:type="textWrapping" w:clear="all""#,
            "textWrapping",
            "all",
        ),
        (r#"w:clear="left""#, "textWrapping", "left"),
        (
            r#"w:clear="right" w:type="textWrapping""#,
            "textWrapping",
            "right",
        ),
        (r#"w:type="page" w:clear="all""#, "page", "all"),
        (r#"w:type="column" w:clear="left""#, "column", "left"),
    ] {
        // An edit of another paragraph, and an edit of the text around the
        // breaks the search reaches.
        let mut doc = doc_with_breaks(attributes);
        doc.paragraph_mut(0).unwrap().add_run(", edited");
        doc.paragraph_mut(3).unwrap().add_run(", edited");
        for (from, to) in [("before", "Before"), ("simple", "Simple")] {
            assert_eq!(doc.replace_text(from, to), 1, "{attributes}: {from}");
        }
        let doc = reopen(&mut doc);
        let xml = doc.part_xml("word/document.xml").unwrap();
        assert_eq!(
            break_kinds_and_clears(&xml),
            vec![(kind.to_owned(), clear.to_owned()); 3],
            "{attributes}: the run and both field results keep the break: {xml}"
        );
        // The edited run and simple field result are written from the
        // model, in Word's spelling; the complex field result keeps its
        // source bytes, even when its paragraph is edited.
        let written = format!(r#"<w:br w:type="{kind}" w:clear="{clear}"/>"#);
        for text in ["Before", "Simple"] {
            let expected = format!("<w:t>{text}</w:t>{written}");
            assert!(xml.contains(&expected), "{attributes}: {expected}: {xml}");
        }
        let source = format!("<w:t>complex</w:t><w:br {attributes}/>");
        assert!(xml.contains(&source), "{attributes}: {source}: {xml}");
    }
}

#[test]
fn a_break_that_clears_nothing_is_written_without_a_clear() {
    // `none` is the default and an unknown value reads as it: both are
    // written as absent, as Word writes a plain break.
    for (attributes, written) in [
        (r#"w:type="textWrapping" w:clear="none""#, "<w:br/>"),
        (r#"w:clear="sideways""#, "<w:br/>"),
        (
            r#"w:type="page" w:clear="none""#,
            r#"<w:br w:type="page"/>"#,
        ),
    ] {
        let mut doc = doc_with_breaks(attributes);
        assert_eq!(doc.replace_text("before", "Before"), 1);
        let doc = reopen(&mut doc);
        let xml = doc.part_xml("word/document.xml").unwrap();
        assert!(
            xml.contains(&format!("<w:t>Before</w:t>{written}")),
            "{attributes}: {written}: {xml}"
        );
    }
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

fn runs_text(doc: &Document, index: usize, runs: std::ops::Range<usize>) -> String {
    let paragraph = doc.paragraph(index).unwrap();
    runs.map(|run| paragraph.run(run).unwrap().text()).collect()
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
fn run_measures_written_with_a_unit_open() {
    // Character spacing and fitText are twips; size, kerning and position
    // half-points. Each takes a universal measure in Transitional.
    let doc = crate::support::doc_with_body(concat!(
        r#"<w:p><w:r><w:rPr><w:spacing w:val="1pt"/><w:kern w:val="8pt"/><w:position w:val="-1pt"/>"#,
        r#"<w:sz w:val="12pt"/><w:fitText w:val="1in" w:id="5"/></w:rPr><w:t>Measured</w:t></w:r></w:p>"#,
    ));
    let paragraph = doc.paragraph(0).unwrap();
    let run = paragraph.run(0).unwrap();
    assert_eq!(run.size(), Some(12.0));
    assert_eq!(run.character_spacing().map(|twips| twips.0), Some(20));
    assert_eq!(run.position(), Some(-2));
    assert_eq!(run.kern(), Some(8.0));
    assert_eq!(run.fit_text().map(|fit| fit.val.0), Some(1440));

    // A text scale written as a percent opens too.
    let doc = crate::support::doc_with_body(
        r#"<w:p><w:r><w:rPr><w:w w:val="150%"/></w:rPr><w:t>Wide</w:t></w:r></w:p>"#,
    );
    assert_eq!(doc.paragraph(0).unwrap().text(), "Wide");
}

#[test]
fn ruby_sizes_written_with_a_unit_read_as_half_points() {
    let doc = crate::support::doc_with_body(concat!(
        r#"<w:p><w:ruby><w:rubyPr><w:hps w:val="5pt"/><w:hpsRaise w:val="9pt"/><w:hpsBaseText w:val="10.5pt"/>"#,
        r#"<w:lid w:val="ja-JP"/></w:rubyPr><w:rt><w:r><w:t>とうきょう</w:t></w:r></w:rt>"#,
        r#"<w:rubyBase><w:r><w:t>東京</w:t></w:r></w:rubyBase></w:ruby></w:p>"#,
    ));
    let paragraph = doc.paragraph(0).unwrap();
    let properties = paragraph.ruby(0).unwrap().properties.as_ref().unwrap();
    assert_eq!(properties.hps.map(|hps| hps.0), Some(10));
    assert_eq!(properties.hps_raise.map(|hps| hps.0), Some(18));
    assert_eq!(properties.hps_base_text.map(|hps| hps.0), Some(21));
}

#[test]
fn run_measures_with_a_unit_pass_the_checked_paragraph_route() {
    let mut doc = doc_with(&["Old"]);
    let location = doc.paragraph_story_location(0).unwrap().unwrap();
    let paragraph = |size: &str| {
        format!(
            r#"<w:p xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:r><w:rPr><w:sz w:val="{size}"/></w:rPr><w:t>New</w:t></w:r></w:p>"#
        )
    };
    assert!(
        doc.replace_paragraph_xml(&location, &paragraph("-1pt"))
            .is_err()
    );
    doc.replace_paragraph_xml(&location, &paragraph("12pt"))
        .unwrap();
    assert_eq!(doc.paragraph(0).unwrap().run(0).unwrap().size(), Some(12.0));
}

#[test]
fn a_carriage_return_in_run_text_reads_back_as_set_after_a_save() {
    let mut doc = doc_with(&["first"]);
    doc.add_paragraph("line\rend");
    doc.paragraph_mut(0).unwrap().add_run("a\rb");
    let saved = doc.to_bytes().unwrap();
    let doc = reopen(&mut Document::from_bytes(&saved).unwrap());
    assert_eq!(texts(&doc), ["firsta\rb", "line\rend"]);
    let xml = doc.part_xml("word/document.xml").unwrap();
    assert!(xml.contains("line&#13;end") && !xml.contains('\r'), "{xml}");
}

#[test]
fn a_raw_carriage_return_in_saved_text_reads_as_a_line_feed_everywhere() {
    // A part written with Windows line ends holds a raw CR LF inside w:t,
    // which every XML reader reads as one line feed; a CR written as a
    // reference stays one.
    let doc = doc_with_body(concat!(
        "<w:p><w:r><w:t>one\r\ntwo\rthree&#13;four</w:t></w:r></w:p>",
        r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r>"#,
        "<w:r><w:instrText xml:space=\"preserve\"> REF a\r\nb </w:instrText></w:r>",
        r#"<w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>r</w:t></w:r>"#,
        r#"<w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#,
    ));
    let expected = "one\ntwo\nthree\rfour";
    assert_eq!(texts(&doc)[0], expected);
    let story = doc.story_paragraphs().unwrap();
    assert_eq!(story[0].text, expected);
    let mut instruction = None;
    for run in doc.paragraphs()[1].runs() {
        for item in run.items() {
            if let RunItemRef::Field(field) = item {
                instruction = Some(field.instruction().to_owned());
            }
        }
    }
    assert_eq!(instruction.as_deref().map(str::trim), Some("REF a\nb"));
}

#[test]
fn an_infallible_setter_drops_a_character_xml_cannot_carry_and_the_document_saves() {
    let bad = "a\u{7}b\u{FFFE}";
    let mut doc = doc_with(&["first"]);
    doc.add_paragraph(bad);
    doc.insert_paragraph(0, bad);
    doc.add_bullet_list_item(bad, 0);
    doc.add_numbered_list_item(bad, 0);
    doc.set_header(bad);
    doc.set_footer(bad);
    doc.set_first_page_header(bad);
    doc.set_first_page_footer(bad);
    doc.append_hyperlink(bad, "https://example.org/\u{7}");
    let relationship = doc.add_hyperlink_relationship("https://example.com/\u{7}");
    let footnote = doc.add_footnote(bad);
    doc.set_title(bad);
    doc.set_author(bad);
    doc.set_subject(bad);
    doc.set_keywords(bad);
    {
        let mut paragraph = doc.paragraph_mut(1).unwrap();
        paragraph.add_ruby(bad, bad);
        paragraph.add_run_inheriting_mark(bad);
        paragraph.add_hyperlink(bad, &relationship);
        paragraph.add_internal_hyperlink(bad, bad, Some(bad));
        paragraph.set_style(bad);
        paragraph.set_shading_pattern(bad, bad, bad);
        paragraph.set_border_all(oxoffice_docx::BorderStyle::Single, 4, bad);
        paragraph.set_border_bottom_with_space(oxoffice_docx::BorderStyle::Single, 4, 2, bad);
        paragraph.set_border_value(
            oxoffice_docx::ParagraphBorderEdge::Left,
            Some((oxoffice_docx::BorderStyle::Single, 4, bad)),
        );
        paragraph.mark().set_font_value(Some(bad));
        paragraph.mark().set_color_value(Some(bad));
        let mut run = paragraph.add_run(bad);
        run.add_text(bad);
        run.add_symbol('\u{7}');
        run.add_symbol_char(bad, 0xF020);
        run.set_font(bad);
        run.set_slot_font(RunFontSlot::ComplexScript, Some(bad));
        run.set_shading_value(Some(bad));
        run.add_picture(
            bad,
            oxoffice_docx::Length::pt(10.0),
            oxoffice_docx::Length::pt(10.0),
        );
        run.set_slot_theme_font(RunFontSlot::EastAsia, Some(bad));
        run.set_font_hint(Some(bad));
        run.set_language(bad);
        run.set_language_east_asia_value(Some(bad));
        run.set_language_bidi_value(Some(bad));
        run.set_color(bad);
        run.set_color_theme(Some(bad), None, None);
        run.set_highlight(bad);
        run.set_style(bad);
    }
    doc.paragraph_mut(2)
        .unwrap()
        .run_mut(0)
        .unwrap()
        .set_text(bad);
    {
        let mut table = doc.add_table(1, 1);
        table.set_style(bad);
        table.set_borders(oxoffice_docx::BorderStyle::Single, 4, bad);
        let mut cell = table.cell(0, 0).unwrap();
        cell.set_text(bad);
        cell.add_paragraph(bad);
        cell.set_shading(bad);
        cell.add_picture(
            bad,
            oxoffice_docx::Length::pt(10.0),
            oxoffice_docx::Length::pt(10.0),
        );
    }
    {
        let mut paragraph = doc.paragraph_mut(3).unwrap();
        paragraph.set_shading(bad);
        paragraph.set_shading_fill_value(Some(bad));
    }
    doc.section_mut(0).unwrap().set_text_direction(bad);
    assert_eq!(doc.replace_text("first", bad), 1);
    assert!(doc.replace_all(&HashMap::from([("ab", bad)])) > 0);

    let saved = doc.to_bytes().expect("the document saves");
    let doc = Document::from_bytes(&saved).unwrap();
    assert_eq!(texts(&doc)[0], "ab");
    assert_eq!(doc.paragraph(2).unwrap().run(0).unwrap().text(), "ab");
    assert_eq!(doc.header_text().as_deref(), Some("ab"));
    assert!(
        doc.footnotes()
            .iter()
            .any(|(id, text)| *id == footnote && text.trim() == "ab")
    );
    assert_eq!(doc.title(), Some("ab"));
    assert_eq!(doc.tables()[0].cell(0, 0).unwrap().text(), "ab\nab\n");
}

#[test]
fn a_checked_setter_refuses_a_character_xml_cannot_carry_and_changes_nothing() {
    let bad = "a\u{7}b";
    let mut doc = doc_with(&["first"]);
    doc.add_table(1, 1);
    let body = doc
        .stories()
        .unwrap()
        .into_iter()
        .find(|story| story.kind() == StoryKind::Body)
        .unwrap();
    let before = doc.to_bytes().unwrap();
    let refusals = [
        doc.paragraph_mut(0).unwrap().set_text(bad).err(),
        doc.paragraph_mut(0)
            .unwrap()
            .add_hyperlink_with_tooltip(bad, "rId1", None)
            .err(),
        doc.table_mut(0).unwrap().set_caption(Some(bad)).err(),
        doc.table_mut(0).unwrap().set_description(Some(bad)).err(),
        doc.set_text_watermark(bad).err(),
        doc.add_style(oxoffice_docx::StyleBuilder::paragraph("Bad", bad))
            .err(),
        doc.add_style(oxoffice_docx::StyleBuilder::paragraph(bad, "Bad"))
            .err(),
        doc.add_style(oxoffice_docx::StyleBuilder::paragraph("Font", "Font").font(bad))
            .err(),
        doc.set_style(oxoffice_docx::StyleBuilder::paragraph("Normal", "Normal").font(bad))
            .err(),
        doc.add_hyperlink_to_story(&body, bad, "https://example.org/")
            .err(),
        doc.add_hyperlink_to_story(&body, "link", "https://example.org/\u{7}")
            .err(),
        doc.add_numbering_definition(&[oxoffice_docx::ListLevel::decimal().level_text(bad)])
            .err(),
    ];
    for (index, refusal) in refusals.into_iter().enumerate() {
        let message = refusal
            .unwrap_or_else(|| panic!("setter {index} accepts U+0007"))
            .to_string();
        assert!(message.contains("U+0007"), "setter {index}: {message}");
    }
    let font = doc
        .add_style(oxoffice_docx::StyleBuilder::paragraph("Font", "Font").font(bad))
        .unwrap_err()
        .to_string();
    assert!(
        font.contains("style Font") && font.contains("w:rFonts/@w:ascii"),
        "the refusal names the value: {font}"
    );
    assert_eq!(doc.to_bytes().unwrap(), before);
}

#[test]
fn a_header_image_background_drops_a_character_xml_cannot_carry() {
    const PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];
    let mut doc = doc_with(&["body"]);
    doc.set_header_image_with_background(
        PNG,
        "logo.png",
        oxoffice_docx::Length::pt(10.0),
        oxoffice_docx::Length::pt(10.0),
        "00\u{7}0000",
    );
    doc.to_bytes().expect("the document saves");
}
