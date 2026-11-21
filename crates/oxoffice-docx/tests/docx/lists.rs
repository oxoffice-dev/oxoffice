//! Bullet and numbered lists: adding items at levels 0 to 8, restarting a
//! count, list definitions and instances (add, read, update, remove), number
//! formats and level templates, styles linked to a list level, the numbering
//! graph check, and the list markers a reader sees, on new documents and on
//! the lists Word, Google Docs, LibreOffice and python-docx saved.

use std::io::Cursor;

use oxoffice_docx::{
    Alignment, Document, ListLevel, ListLevelRestart, ListLevelSuffix, ListNumberFormat,
    NumberedOutlineNode, NumberingLevelOverride, StyleBuilder, Twips,
};
use oxoffice_opc::OpcPackage;

use crate::support::{corpus_doc, doc_with, reopen, texts};

/// The bullet glyph Word, LibreOffice and python-docx store for a Symbol
/// font bullet: a private-use character, drawn as a dot in that font.
const SYMBOL_BULLET: &str = "\u{f0b7}";

/// The marker a reader sees before each body paragraph, as layout draws it,
/// in paragraph order: "" for a paragraph that is not a list item.
fn markers(doc: &Document) -> Vec<String> {
    let layout = doc.layout_deterministic().expect("document lays out");
    (0..doc.paragraph_count())
        .map(|index| {
            doc.content_index_of_paragraph(index)
                .and_then(|body| layout.document_body_paragraph_numbering(body))
                .map(|numbering| numbering.marker_text.clone())
                .unwrap_or_default()
        })
        .collect()
}

/// The list items of the body as a reader sees them: marker, space, text.
fn list_lines(doc: &Document) -> Vec<String> {
    markers(doc)
        .into_iter()
        .zip(texts(doc))
        .filter(|(marker, _)| !marker.is_empty())
        .map(|(marker, text)| format!("{marker} {text}"))
        .collect()
}

/// A new document with one list: a definition of these levels and one
/// instance of it. Returns the instance id, which paragraphs take.
fn single_list(levels: &[ListLevel]) -> (Document, u32) {
    let mut doc = Document::new();
    let definition = doc
        .add_numbering_definition(levels)
        .expect("levels are valid");
    let id = doc
        .add_numbering_instance(definition, &[])
        .expect("definition exists");
    (doc, id)
}

/// The definition an instance uses.
fn definition_of(doc: &Document, id: u32) -> u32 {
    doc.numbering_instance(id)
        .expect("instance exists")
        .definition_id
}

/// Append one paragraph per level, on list `id`, named "item 1", "item 2"...
fn add_items(doc: &mut Document, id: u32, levels: &[u32]) {
    for (index, level) in levels.iter().enumerate() {
        doc.add_paragraph(&format!("item {}", index + 1))
            .numbering(id, *level);
    }
}

/// The index of the first paragraph whose text starts with `prefix`.
fn paragraph_starting(doc: &Document, prefix: &str) -> usize {
    texts(doc)
        .iter()
        .position(|text| text.starts_with(prefix))
        .unwrap_or_else(|| panic!("no paragraph starts with {prefix:?}"))
}

/// The list items of the corpus report, with the bullet glyph its producer
/// draws.
fn report_list_lines(bullet: &str) -> Vec<String> {
    let points = [
        "Subscription revenue reached 4.2 million euros, up from 3.7 million.",
        "Churn fell to 2.1 % per month, its lowest level in two years.",
        "The Lyon warehouse opened on schedule and serves the southern region.",
    ];
    let steps = [
        "Hire two support engineers for the evening shift.",
        "Move the remaining customers to the new billing system.",
        "Review warehouse contracts before the January renewal.",
    ];
    points
        .iter()
        .map(|text| format!("{bullet} {text}"))
        .chain(
            steps
                .iter()
                .enumerate()
                .map(|(index, text)| format!("{}. {text}", index + 1)),
        )
        .collect()
}

/// A document whose Heading1 and Heading2 styles are linked to levels 0 and
/// 1 of one list numbered "1", "1.1", with a plain paragraph in the middle.
fn numbered_headings() -> (Document, u32) {
    let (mut doc, id) = single_list(&[
        ListLevel::decimal().level_text("%1"),
        ListLevel::decimal().level_text("%1.%2"),
    ]);
    doc.link_style_to_numbering("Heading1", id, 0).unwrap();
    doc.link_style_to_numbering("Heading2", id, 1).unwrap();
    for (text, style) in [
        ("Introduction", "Heading1"),
        ("Scope", "Heading2"),
        ("Terms", "Heading2"),
        ("Body text", "Normal"),
        ("Method", "Heading1"),
        ("Data", "Heading2"),
    ] {
        doc.add_paragraph(text).style(style);
    }
    (doc, id)
}

// Adding items.

#[test]
fn bullet_items_share_one_list_and_survive_a_save() {
    let mut doc = doc_with(&["Items:"]);
    doc.add_bullet_list_item("First", 0);
    doc.add_bullet_list_item("Nested", 1);
    doc.add_bullet_list_item("Deeper", 2);
    doc.add_bullet_list_item("Second", 0);

    let doc = reopen(&mut doc);
    assert_eq!(
        texts(&doc),
        ["Items:", "First", "Nested", "Deeper", "Second"]
    );
    let id = doc.paragraph(1).unwrap().numbering().unwrap().0;
    let numbering: Vec<_> = doc.paragraphs().iter().map(|p| p.numbering()).collect();
    assert_eq!(
        numbering,
        [
            None,
            Some((id, 0)),
            Some((id, 1)),
            Some((id, 2)),
            Some((id, 0))
        ]
    );
    assert_eq!(doc.numbering_is_bullet(id), Some(true));
    assert_eq!(markers(&doc), ["", "•", "◦", "▪", "•"]);
}

#[test]
fn numbered_items_count_per_level_with_letters_and_roman_numerals_below() {
    let mut doc = Document::new();
    for (text, level) in [
        ("Step one", 0),
        ("Step two", 0),
        ("Sub a", 1),
        ("Sub b", 1),
        ("Detail", 2),
        ("Step three", 0),
    ] {
        doc.add_numbered_list_item(text, level);
    }

    let doc = reopen(&mut doc);
    let id = doc.paragraph(0).unwrap().numbering().unwrap().0;
    assert_eq!(doc.numbering_is_bullet(id), Some(false));
    assert_eq!(markers(&doc), ["1.", "2.", "a.", "b.", "i.", "3."]);
}

#[test]
fn bullets_and_numbers_use_two_lists_and_a_count_goes_on_past_other_paragraphs() {
    let mut doc = Document::new();
    doc.add_numbered_list_item("one", 0);
    doc.add_bullet_list_item("aside", 0);
    doc.add_paragraph("plain");
    doc.add_numbered_list_item("two", 0);

    let doc = reopen(&mut doc);
    assert_eq!(markers(&doc), ["1.", "•", "", "2."]);
    let numbering: Vec<_> = doc.paragraphs().iter().map(|p| p.numbering()).collect();
    assert_eq!(numbering[0], numbering[3]);
    assert_ne!(numbering[0], numbering[1]);
    assert_eq!(doc.numbering_definitions().len(), 2);
    assert_eq!(doc.numbering_instances().len(), 2);
}

#[test]
fn list_levels_zero_to_eight_are_accepted_and_nine_is_refused() {
    for numbered in [false, true] {
        let mut doc = Document::new();
        for level in 0..9 {
            if numbered {
                doc.try_add_numbered_list_item("item", level).unwrap();
            } else {
                doc.try_add_bullet_list_item("item", level).unwrap();
            }
        }
        let before = doc.to_bytes().unwrap();
        let refused = if numbered {
            doc.try_add_numbered_list_item("too deep", 9).map(|_| ())
        } else {
            doc.try_add_bullet_list_item("too deep", 9).map(|_| ())
        };
        let error = refused.unwrap_err().to_string();
        assert!(error.contains("between 0 and 8"), "{error}");
        let id = doc.paragraph(0).unwrap().numbering().unwrap().0;
        assert!(!doc.paragraph_mut(0).unwrap().set_numbering(id, 9));
        assert_eq!(doc.to_bytes().unwrap(), before, "a refusal changes nothing");

        let doc = reopen(&mut doc);
        let levels: Vec<_> = doc
            .paragraphs()
            .iter()
            .map(|p| p.numbering().unwrap().1)
            .collect();
        assert_eq!(levels, (0..9).collect::<Vec<_>>());
        assert_eq!(doc.numbering_definitions().len(), 1);
        assert_eq!(doc.numbering_instances().len(), 1);
        doc.validate_numbering_graph().unwrap();
    }
}

#[test]
fn list_text_xml_cannot_carry_is_refused_without_a_change() {
    let mut doc = doc_with(&["before"]);
    let before = doc.to_bytes().unwrap();
    for text in ["nul \u{0} inside", "bell \u{7}"] {
        assert!(doc.try_add_bullet_list_item(text, 0).is_err(), "{text:?}");
        assert!(doc.try_add_numbered_list_item(text, 0).is_err(), "{text:?}");
    }
    assert_eq!(doc.to_bytes().unwrap(), before);
    assert_eq!(texts(&doc), ["before"]);
    assert!(doc.numbering_definitions().is_empty());
}

#[test]
fn list_items_keep_cjk_rtl_emoji_and_empty_text() {
    let mut doc = Document::new();
    doc.add_bullet_list_item("第一项", 0);
    doc.add_bullet_list_item("البند الأول", 0);
    doc.add_numbered_list_item("😀 emoji", 0);
    doc.add_numbered_list_item("", 0);

    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["第一项", "البند الأول", "😀 emoji", ""]);
    // An empty item is still a list item with its marker.
    assert_eq!(markers(&doc), ["•", "•", "1.", "2."]);
}

#[test]
fn clearing_a_paragraphs_numbering_makes_it_plain_again() {
    let mut doc = Document::new();
    for text in ["one", "two", "three"] {
        doc.add_numbered_list_item(text, 0);
    }
    let mut doc = reopen(&mut doc);
    assert!(doc.paragraph_mut(1).unwrap().set_numbering_value(None));

    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["one", "two", "three"]);
    assert_eq!(doc.paragraph(1).unwrap().numbering(), None);
    assert_eq!(markers(&doc), ["1.", "", "2."]);
}

// Restarting a count.

#[test]
fn restarting_while_adding_starts_a_new_count_that_later_items_follow() {
    let mut doc = Document::new();
    doc.add_numbered_list_item("one", 0);
    doc.add_numbered_list_item("two", 0);
    doc.try_add_numbered_list_item_with_restart("again one", 0, true)
        .unwrap();
    doc.add_numbered_list_item("again two", 0);
    let numbering: Vec<_> = doc.paragraphs().iter().map(|p| p.numbering()).collect();
    assert_ne!(numbering[2], numbering[1]);
    assert_eq!(numbering[3], numbering[2]);
    assert_eq!(markers(&doc), ["1.", "2.", "1.", "2."]);

    // The next session goes on with the restarted count.
    let mut doc = reopen(&mut doc);
    doc.add_numbered_list_item("again three", 0);
    assert_eq!(markers(&doc), ["1.", "2.", "1.", "2.", "3."]);
}

#[test]
fn restart_numbering_gives_one_item_a_new_start_and_the_items_after_follow() {
    let mut doc = Document::new();
    for text in ["a", "b", "c", "d"] {
        doc.add_numbered_list_item(text, 0);
    }
    let old = doc.paragraph(0).unwrap().numbering().unwrap().0;
    let new = doc.restart_numbering(2, 7).unwrap();
    assert_ne!(new, old);
    // Only the named paragraph moves to the new instance; the next one
    // stays on the old instance and counts on from the restarted item.
    assert_eq!(doc.paragraph(2).unwrap().numbering(), Some((new, 0)));
    assert_eq!(doc.paragraph(3).unwrap().numbering(), Some((old, 0)));

    let doc = reopen(&mut doc);
    assert_eq!(markers(&doc), ["1.", "2.", "7.", "8."]);
    let instance = doc.numbering_instance(new).unwrap();
    assert_eq!(instance.level_overrides.len(), 1);
    assert_eq!(instance.level_overrides[0].level, 0);
    assert_eq!(instance.level_overrides[0].start, Some(7));
}

#[test]
fn restart_numbering_refuses_bullets_plain_paragraphs_and_missing_ones() {
    let mut doc = Document::new();
    doc.add_bullet_list_item("bullet", 0);
    doc.add_paragraph("plain");
    let before = doc.to_bytes().unwrap();
    for (paragraph, message) in [
        (0, "numbered non-bullet level"),
        (1, "no explicit numbering"),
        (5, "direct body paragraph"),
    ] {
        let error = doc.restart_numbering(paragraph, 1).unwrap_err().to_string();
        assert!(error.contains(message), "paragraph {paragraph}: {error}");
    }
    assert_eq!(doc.to_bytes().unwrap(), before);
}

// List definitions and instances.

#[test]
fn each_list_definition_counts_on_its_own() {
    let mut doc = Document::new();
    let first = doc.add_list_definition(&[ListLevel::decimal()]);
    let second = doc.add_list_definition(&[ListLevel::decimal()]);
    assert!(first != 0 && second != 0 && first != second);
    doc.add_paragraph("first list, one").numbering(first, 0);
    doc.add_paragraph("second list, one").numbering(second, 0);
    doc.add_paragraph("first list, two").numbering(first, 0);

    let doc = reopen(&mut doc);
    assert_eq!(markers(&doc), ["1.", "1.", "2."]);
}

#[test]
fn one_definition_can_mix_bullets_and_numbers_across_levels() {
    let mut doc = Document::new();
    let id = doc.add_list_definition(&[ListLevel::bullet(), ListLevel::decimal().start(3)]);
    add_items(&mut doc, id, &[0, 1, 1]);

    let doc = reopen(&mut doc);
    assert_eq!(markers(&doc), ["•", "3.", "4."]);
    // Level 0 decides whether the list reads as a bullet list.
    assert_eq!(doc.numbering_is_bullet(id), Some(true));
    let nested = doc.numbering_level(id, 1).unwrap();
    assert_eq!((nested.format_name, nested.start), ("decimal", 3));
}

#[test]
fn number_formats_show_the_markers_word_shows_and_keep_their_names() {
    use ListNumberFormat::*;
    for (format, expected) in [
        (Decimal, ["1.", "2.", "3.", "4."]),
        (UpperLetter, ["A.", "B.", "C.", "D."]),
        (LowerLetter, ["a.", "b.", "c.", "d."]),
        (UpperRoman, ["I.", "II.", "III.", "IV."]),
        (LowerRoman, ["i.", "ii.", "iii.", "iv."]),
        (Bullet, ["•", "•", "•", "•"]),
    ] {
        let (mut doc, id) = single_list(&[ListLevel::new(format.clone())]);
        add_items(&mut doc, id, &[0, 0, 0, 0]);
        let doc = reopen(&mut doc);
        assert_eq!(markers(&doc), expected, "{format:?}");
        assert_eq!(doc.numbering_is_bullet(id), Some(format == Bullet));
    }

    // Formats the layout draws no marker for still keep their exact name.
    assert_eq!(ListNumberFormat::from_name("lowerLetter"), LowerLetter);
    assert_eq!(
        ListNumberFormat::from_name("Decimal"),
        Other("Decimal".to_owned()),
        "format names are case-sensitive"
    );
    for name in ["decimalZero", "ordinal", "cardinalText", "chicago"] {
        let (mut doc, id) = single_list(&[ListLevel::new(ListNumberFormat::from_name(name))]);
        add_items(&mut doc, id, &[0]);
        let doc = reopen(&mut doc);
        assert_eq!(doc.numbering_level(id, 0).unwrap().format_name, name);
        assert_eq!(doc.numbering_is_bullet(id), Some(false));
    }
}

#[test]
fn level_text_combines_parent_numbers_and_legal_numbering_turns_them_decimal() {
    let (mut doc, id) = single_list(&[
        ListLevel::new(ListNumberFormat::UpperRoman).level_text("%1."),
        ListLevel::decimal()
            .level_text("%1.%2.")
            .legal_numbering(true),
        ListLevel::decimal().level_text("%1.%2.%3"),
    ]);
    add_items(&mut doc, id, &[0, 1, 1, 2, 2, 0, 1, 2]);

    let doc = reopen(&mut doc);
    // Level 1 is legal, so its parent shows as "1" there, and stays "I" at
    // level 2, which is not.
    assert_eq!(
        markers(&doc),
        [
            "I.", "1.1.", "1.2.", "I.2.1", "I.2.2", "II.", "2.1.", "II.1.1"
        ]
    );
    assert_eq!(
        doc.numbering_level(id, 1).unwrap().legal_numbering,
        Some(true)
    );
}

#[test]
fn a_nested_level_restarts_under_each_parent_unless_told_never_to() {
    let lettered = ListLevel::new(ListNumberFormat::LowerLetter);
    for (nested, expected) in [
        (lettered.clone(), ["1.", "a.", "b.", "2.", "a."]),
        (
            lettered.restart(ListLevelRestart::Never),
            ["1.", "a.", "b.", "2.", "c."],
        ),
    ] {
        let (mut doc, id) = single_list(&[ListLevel::decimal(), nested]);
        add_items(&mut doc, id, &[0, 1, 1, 0, 1]);
        let doc = reopen(&mut doc);
        assert_eq!(markers(&doc), expected);
    }
}

#[test]
fn instances_of_one_definition_can_start_at_different_numbers() {
    let (mut doc, first) = single_list(&[ListLevel::decimal().start(3)]);
    let definition = definition_of(&doc, first);
    let second = doc
        .add_numbering_instance(definition, &[NumberingLevelOverride::new(0).start(10)])
        .unwrap();
    add_items(&mut doc, first, &[0, 0]);
    add_items(&mut doc, second, &[0, 0]);

    let doc = reopen(&mut doc);
    assert_eq!(markers(&doc), ["3.", "4.", "10.", "11."]);
    let instance = doc.numbering_instance(second).unwrap();
    assert_eq!(instance.definition_id, definition);
    assert_eq!(instance.level_overrides[0].start, Some(10));
    assert_eq!(doc.numbering_instance(first).unwrap().level_overrides, []);
}

#[test]
fn updating_a_definition_or_an_instance_changes_the_markers_of_its_lists() {
    let (mut doc, first) = single_list(&[ListLevel::decimal().start(3)]);
    let definition = definition_of(&doc, first);
    let second = doc
        .add_numbering_instance(definition, &[NumberingLevelOverride::new(0).start(10)])
        .unwrap();
    add_items(&mut doc, first, &[0, 0]);
    add_items(&mut doc, second, &[0, 0]);

    doc.update_numbering_instance(
        second,
        definition,
        &[NumberingLevelOverride::new(0).start(20)],
    )
    .unwrap();
    // Edit the level as read: the format and template change, the start of
    // 3 stays.
    let mut levels = doc.numbering_definition(definition).unwrap().levels;
    levels[0].properties.format = ListNumberFormat::UpperRoman;
    levels[0].properties = levels[0].properties.clone().level_text("(%1)");
    doc.update_numbering_definition(definition, &levels)
        .unwrap();

    let doc = reopen(&mut doc);
    assert_eq!(markers(&doc), ["(III)", "(IV)", "(XX)", "(XXI)"]);
    let level = &doc.numbering_definition(definition).unwrap().levels[0].properties;
    assert_eq!(level.format, ListNumberFormat::UpperRoman);
    assert_eq!(level.level_text_value(), Some("(%1)"));
    assert_eq!(level.start, Some(3));
    doc.validate_numbering_graph().unwrap();
}

#[test]
fn only_a_list_nothing_uses_can_be_removed() {
    let (mut doc, id) = single_list(&[ListLevel::decimal()]);
    let definition = definition_of(&doc, id);
    doc.add_paragraph("item").numbering(id, 0);

    let before = doc.to_bytes().unwrap();
    let error = doc.remove_numbering_instance(id).unwrap_err().to_string();
    assert!(error.contains("referenced by document content"), "{error}");
    let error = doc
        .remove_numbering_definition(definition)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains(&format!("referenced by instance {id}")),
        "{error}"
    );
    assert_eq!(doc.to_bytes().unwrap(), before);

    doc.paragraph_mut(0).unwrap().set_numbering_value(None);
    assert!(doc.remove_numbering_instance(id).unwrap());
    assert!(doc.remove_numbering_definition(definition).unwrap());
    assert!(!doc.remove_numbering_instance(id).unwrap(), "already gone");

    let doc = reopen(&mut doc);
    assert!(doc.numbering_instances().is_empty());
    assert!(doc.numbering_definitions().is_empty());
    assert_eq!(texts(&doc), ["item"]);
    doc.validate_numbering_graph().unwrap();
}

#[test]
fn numbering_readers_answer_nothing_for_unknown_ids() {
    let fresh = Document::new();
    assert!(fresh.numbering_definitions().is_empty());
    assert!(fresh.numbering_instances().is_empty());
    fresh.validate_numbering_graph().unwrap();

    let (mut doc, id) = single_list(&[ListLevel::decimal()]);
    assert_eq!(doc.numbering_is_bullet(99), None);
    assert!(doc.numbering_level(0, 0).is_none(), "0 means no list");
    assert!(doc.numbering_level(id, 1).is_none(), "a one-level list");
    assert!(doc.numbering_instance(99).is_none());
    assert!(doc.numbering_definition(99).is_none());
    assert!(!doc.set_list_level(99, 0, ListLevel::bullet()));
    assert!(!doc.remove_numbering_instance(99).unwrap());
    assert!(!doc.remove_numbering_definition(99).unwrap());
}

#[test]
fn level_settings_read_back_after_a_save() {
    let (mut doc, id) = single_list(&[ListLevel::new(ListNumberFormat::UpperLetter)
        .start(4)
        .level_text("%1)")
        .suffix(ListLevelSuffix::Space)
        .alignment(Alignment::Right)
        .indentation(Some(Twips(1080)), Some(Twips(540)), None)]);
    add_items(&mut doc, id, &[0, 0]);

    let doc = reopen(&mut doc);
    assert_eq!(markers(&doc), ["D)", "E)"]);
    let level = doc.numbering_level(id, 0).unwrap();
    assert_eq!(level.format_name, "upperLetter");
    assert_eq!(level.start, 4);
    assert_eq!(level.level_text, Some("%1)"));
    assert_eq!(level.suffix, ListLevelSuffix::Space);
    assert_eq!(level.alignment, Some(Alignment::Right));
    assert_eq!(level.indent_left, Some(Twips(1080)));
    assert_eq!(level.indent_hanging, Some(Twips(540)));
    let definition = doc.numbering_definition(definition_of(&doc, id)).unwrap();
    let properties = &definition.levels[0].properties;
    assert_eq!(properties.suffix_value(), Some(ListLevelSuffix::Space));
    assert_eq!(
        properties.indentation_value(),
        (Some(Twips(1080)), Some(Twips(540)), None)
    );
}

#[test]
fn invalid_list_levels_are_refused_without_a_change() {
    let mut doc = Document::new();
    let before = doc.to_bytes().unwrap();
    let both_indents = ListLevel::decimal().indentation(None, Some(Twips(360)), Some(Twips(360)));
    for (levels, message) in [
        (vec![], "require 1 through 9 levels"),
        (vec![ListLevel::decimal(); 10], "require 1 through 9 levels"),
        (
            vec![ListLevel::decimal().level_text("%2.")],
            "invalid placeholder %2",
        ),
        (vec![both_indents], "both hanging and first-line"),
        (
            vec![ListLevel::decimal().restart(ListLevelRestart::After(0))],
            "cannot restart after level 0",
        ),
        (
            vec![ListLevel::decimal().template_code("xyz")],
            "eight hexadecimal digits",
        ),
    ] {
        let error = doc
            .add_numbering_definition(&levels)
            .unwrap_err()
            .to_string();
        assert!(error.contains(message), "{error}");
    }
    // The older helper answers 0 instead of an error.
    assert_eq!(
        doc.add_list_definition(&[ListLevel::decimal().restart(ListLevelRestart::After(0))]),
        0
    );
    let error = doc.add_numbering_instance(77, &[]).unwrap_err().to_string();
    assert!(error.contains("definition 77 does not exist"), "{error}");
    assert_eq!(doc.to_bytes().unwrap(), before);
}

#[test]
fn set_list_level_redefines_a_deeper_level_after_the_fact() {
    let mut doc = Document::new();
    let id = doc.add_list_definition(&[ListLevel::bullet()]);
    assert!(doc.set_list_level(id, 1, ListLevel::decimal().start(3)));
    assert!(
        !doc.set_list_level(id, 9, ListLevel::decimal()),
        "no level 9"
    );
    assert!(
        !doc.set_list_level(
            id,
            1,
            ListLevel::decimal().restart(ListLevelRestart::After(1))
        ),
        "an invalid level"
    );
    add_items(&mut doc, id, &[0, 1, 1]);

    let doc = reopen(&mut doc);
    assert_eq!(markers(&doc), ["•", "3.", "4."]);
    let level = doc.numbering_level(id, 1).unwrap();
    assert_eq!((level.format_name, level.start), ("decimal", 3));
}

// List text as a reader sees it.

#[test]
fn markdown_writes_bullets_numbers_nesting_and_start_numbers() {
    let mut doc = doc_with(&["Intro"]);
    doc.add_bullet_list_item("point", 0);
    doc.add_bullet_list_item("sub point", 1);
    doc.add_numbered_list_item("step", 0);
    doc.add_numbered_list_item("sub step", 1);
    let markdown = doc.to_markdown();
    assert!(markdown.contains("- point\n  - sub point\n"), "{markdown}");
    assert!(markdown.contains("1. step\n   1. sub step\n"), "{markdown}");

    let (mut doc, id) = single_list(&[ListLevel::decimal().start(3)]);
    add_items(&mut doc, id, &[0]);
    let markdown = doc.to_markdown();
    assert!(markdown.contains("3. item 1"), "{markdown}");

    // Google Docs numbers list paragraphs directly, and they export as lists.
    let markdown = corpus_doc("google/report.docx").to_markdown();
    for line in [
        "- Subscription revenue reached 4.2 million euros, up from 3.7 million.",
        "1. Hire two support engineers for the evening shift.",
    ] {
        assert!(markdown.lines().any(|l| l == line), "{line}\n{markdown}");
    }
}

// Styles linked to a list level.

#[test]
fn a_style_linked_to_a_list_level_numbers_every_paragraph_of_that_style() {
    let (mut doc, id) = numbered_headings();

    let doc = reopen(&mut doc);
    assert_eq!(markers(&doc), ["1", "1.1", "1.2", "", "2", "2.1"]);
    // The number comes from the style: the paragraphs carry none.
    assert_eq!(doc.paragraph(0).unwrap().numbering(), None);
    let style = doc.style("Heading2").unwrap();
    let properties = style.paragraph_properties().unwrap();
    assert_eq!(
        (properties.num_id, properties.num_ilvl),
        (Some(id), Some(1))
    );
    assert_eq!(
        doc.numbering_level(id, 1).unwrap().paragraph_style,
        Some("Heading2")
    );
    doc.validate_numbering_graph().unwrap();
}

#[test]
fn an_instance_update_keeps_the_style_links_of_its_levels() {
    // Heading1 and Heading2 are linked both ways to the list's levels; an
    // update that would drop the levels' names of them is refused.
    let (mut doc, id) = numbered_headings();
    let plain = doc
        .add_numbering_definition(&[ListLevel::decimal(), ListLevel::decimal()])
        .unwrap();
    let before = doc.to_bytes().unwrap();
    let error = doc
        .update_numbering_instance(id, plain, &[])
        .unwrap_err()
        .to_string();
    assert!(error.contains("style-link"), "{error}");
    assert_eq!(doc.to_bytes().unwrap(), before);
    doc.update_numbering_instance(
        id,
        definition_of(&doc, id),
        &[NumberingLevelOverride::new(0).start(3)],
    )
    .unwrap();
    assert_eq!(
        markers(&doc)[0],
        "3",
        "an update that keeps the links goes through"
    );
}

#[test]
fn an_instance_update_keeps_a_style_link_held_by_a_swapped_in_level() {
    let (mut doc, numbered) = single_list(&[ListLevel::decimal()]);
    let swapped = doc
        .add_numbering_instance(
            definition_of(&doc, numbered),
            &[NumberingLevelOverride::new(0).replacement(ListLevel::bullet())],
        )
        .unwrap();
    doc.add_style(StyleBuilder::paragraph("Points", "Points"))
        .unwrap();
    doc.link_style_to_numbering("Points", swapped, 0).unwrap();
    let link = doc.numbering_instance(swapped).unwrap().level_overrides[0]
        .paragraph_style_link
        .clone();
    assert_eq!(
        link.as_deref(),
        Some("Points"),
        "the swapped-in level holds the link"
    );

    let before = doc.to_bytes().unwrap();
    let error = doc
        .update_numbering_instance(swapped, definition_of(&doc, swapped), &[])
        .unwrap_err()
        .to_string();
    assert!(error.contains("style-link"), "{error}");
    assert_eq!(doc.to_bytes().unwrap(), before);
}

#[test]
fn a_level_that_names_a_style_not_numbered_by_it_is_refused() {
    // The other way round from LibreOffice's one-way link: the level names
    // Heading2, whose style no longer points at any list.
    let (mut doc, _) = numbered_headings();
    let mut package =
        oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(doc.to_bytes().unwrap()))
            .expect("package opens");
    let xml = String::from_utf8(package.get_part("/word/styles.xml").unwrap().to_vec()).unwrap();
    let style = xml
        .find(r#"w:styleId="Heading2""#)
        .expect("the Heading2 style");
    let start = style + xml[style..].find("<w:numPr>").expect("its numbering");
    let end = start + xml[start..].find("</w:numPr>").unwrap() + "</w:numPr>".len();
    let xml = format!("{}{}", &xml[..start], &xml[end..]);
    package.set_part("/word/styles.xml", xml.into_bytes());
    let mut bytes = std::io::Cursor::new(Vec::new());
    package.write_to(&mut bytes).unwrap();
    let doc = Document::from_bytes(&bytes.into_inner()).unwrap();

    let error = doc.validate_numbering_graph().unwrap_err().to_string();
    assert!(
        error.contains("and style 'Heading2' are not reciprocally linked"),
        "{error}"
    );
}

#[test]
fn headings_linked_to_a_list_show_their_numbers_in_the_outline() {
    fn flatten(nodes: &[NumberedOutlineNode], out: &mut Vec<String>) {
        for node in nodes {
            out.push(node.display_text.clone());
            flatten(&node.children, out);
        }
    }
    let (doc, _) = numbered_headings();
    let outline = doc.numbered_document_outline().unwrap();
    assert_eq!(outline.len(), 2, "two top-level headings");
    assert_eq!(outline[0].number.as_deref(), Some("1"));
    assert_eq!(outline[0].text, "Introduction");
    let mut lines = Vec::new();
    flatten(&outline, &mut lines);
    assert_eq!(
        lines,
        [
            "1 Introduction",
            "1.1 Scope",
            "1.2 Terms",
            "2 Method",
            "2.1 Data"
        ]
    );
}

#[test]
fn unlinking_a_style_stops_numbering_its_paragraphs() {
    let (mut doc, id) = numbered_headings();
    let mut doc = reopen(&mut doc);
    doc.unlink_style_from_numbering("Heading2", id, 1).unwrap();
    let error = doc
        .unlink_style_from_numbering("Heading1", id, 1)
        .unwrap_err()
        .to_string();
    assert!(error.contains("are not linked"), "{error}");

    let doc = reopen(&mut doc);
    assert_eq!(markers(&doc), ["1", "", "", "", "2", ""]);
    let properties = doc
        .style("Heading2")
        .unwrap()
        .paragraph_properties()
        .cloned();
    assert_eq!(
        properties.map(|p| (p.num_id, p.num_ilvl)),
        Some((None, None))
    );
    assert_eq!(doc.numbering_level(id, 1).unwrap().paragraph_style, None);
    doc.validate_numbering_graph().unwrap();
}

#[test]
fn linking_a_style_refuses_conflicts_and_styles_that_cannot_hold_a_list() {
    let (mut doc, id) = numbered_headings();
    doc.add_style(StyleBuilder::character("Emphasis", "Emphasis"))
        .unwrap();
    let before = doc.to_bytes().unwrap();
    for (style, level, instance, message) in [
        (
            "Heading1",
            1,
            id,
            "already references a different numbering level",
        ),
        ("Heading3", 0, id, "already linked to another style"),
        ("Emphasis", 0, id, "is not a paragraph style"),
        ("Missing", 0, id, "does not exist"),
        ("Heading3", 0, 99, "numbering instance 99 does not exist"),
    ] {
        let error = doc
            .link_style_to_numbering(style, instance, level)
            .unwrap_err()
            .to_string();
        assert!(error.contains(message), "{style} {level}: {error}");
    }
    assert_eq!(doc.to_bytes().unwrap(), before);
}

// The numbering graph.

#[test]
fn a_paragraph_on_a_missing_list_fails_validation_but_saves_unnumbered() {
    let mut doc = doc_with(&["points nowhere"]);
    doc.paragraph_mut(0).unwrap().set_numbering(42, 0);
    let error = doc.validate_numbering_graph().unwrap_err().to_string();
    assert!(error.contains("missing numbering instance 42"), "{error}");

    let doc = reopen(&mut doc);
    assert_eq!(doc.paragraph(0).unwrap().numbering(), Some((42, 0)));
    assert_eq!(markers(&doc), [""]);
}

// Lists of the corpus reports.

#[test]
fn word_numbers_list_paragraphs_through_their_style_and_google_directly() {
    // Word for Mac in French names List Bullet "Listepuces" and List Number
    // "Listenumros", and puts the numbering on those styles.
    let word = corpus_doc("word/report.docx");
    for (prefix, style, bullet) in [
        ("Subscription revenue", "Listepuces", true),
        ("Hire two", "Listenumros", false),
    ] {
        let paragraph = word.paragraph(paragraph_starting(&word, prefix)).unwrap();
        assert_eq!(paragraph.style_id(), Some(style));
        assert_eq!(paragraph.numbering(), None, "no direct numbering");
        let effective = word.effective_paragraph_properties(&paragraph);
        let (id, level) = (effective.num_id.unwrap(), effective.num_ilvl.unwrap());
        assert_eq!(word.numbering_is_bullet(id), Some(bullet));
        assert_eq!(
            word.numbering_level(id, level).unwrap().paragraph_style,
            Some(style)
        );
    }

    let google = corpus_doc("google/report.docx");
    let paragraph = google
        .paragraph(paragraph_starting(&google, "Subscription revenue"))
        .unwrap();
    let (id, level) = paragraph.numbering().expect("direct numbering");
    assert_eq!(google.numbering_is_bullet(id), Some(true));
    assert_eq!(
        google.numbering_level(id, level).unwrap().level_text,
        Some("●")
    );
}

#[test]
fn list_items_of_every_producer_read_the_same_after_a_save() {
    for (name, bullet) in [
        ("word/report.docx", SYMBOL_BULLET),
        ("google/report.docx", "●"),
        ("libreoffice/report.docx", SYMBOL_BULLET),
        ("authored/report.docx", SYMBOL_BULLET),
    ] {
        let mut doc = corpus_doc(name);
        assert_eq!(list_lines(&doc), report_list_lines(bullet), "{name}");
        let definitions = doc.numbering_definitions();
        let instances = doc.numbering_instances();

        let doc = reopen(&mut doc);
        assert_eq!(list_lines(&doc), report_list_lines(bullet), "{name} saved");
        assert_eq!(doc.numbering_definitions(), definitions, "{name}");
        assert_eq!(doc.numbering_instances(), instances, "{name}");
    }
}

#[test]
fn items_added_to_the_corpus_reports_continue_their_lists() {
    for (name, bullet) in [
        ("word/report.docx", SYMBOL_BULLET),
        ("google/report.docx", "●"),
        ("libreoffice/report.docx", SYMBOL_BULLET),
        ("authored/report.docx", SYMBOL_BULLET),
    ] {
        let mut doc = corpus_doc(name);
        let definitions = doc.numbering_definitions().len();
        doc.try_add_bullet_list_item("New point", 0).unwrap();
        doc.try_add_numbered_list_item("New step", 0).unwrap();

        let doc = reopen(&mut doc);
        // Each new item joins the list of the report's items of its kind, at their level.
        let list_of = |prefix: &str| {
            let paragraph = doc.paragraph(paragraph_starting(&doc, prefix)).unwrap();
            let effective = doc.effective_paragraph_properties(&paragraph);
            (effective.num_id, effective.num_ilvl.unwrap_or(0))
        };
        assert_eq!(
            list_of("New point"),
            list_of("Subscription revenue"),
            "{name}"
        );
        assert_eq!(list_of("New step"), list_of("Hire two"), "{name}");
        let mut expected = report_list_lines(bullet);
        expected.extend([format!("{bullet} New point"), "4. New step".to_owned()]);
        assert_eq!(list_lines(&doc), expected, "{name}");
        assert_eq!(
            doc.numbering_definitions().len(),
            definitions,
            "{name}: the report's own lists are reused"
        );
        doc.validate_numbering_graph().unwrap();
    }
}

#[test]
fn roman_markers_go_up_to_3999_then_show_the_number() {
    for (format, start, expected) in [
        (
            ListNumberFormat::UpperRoman,
            3998,
            ["MMMCMXCVIII.", "MMMCMXCIX.", "4000."],
        ),
        (
            ListNumberFormat::LowerRoman,
            3998,
            ["mmmcmxcviii.", "mmmcmxcix.", "4000."],
        ),
    ] {
        let (mut doc, id) = single_list(&[ListLevel::new(format.clone()).start(start)]);
        add_items(&mut doc, id, &[0; 3]);
        assert_eq!(markers(&doc), expected, "{format:?}");
    }
    // A hostile start value draws a short marker.
    let (mut doc, id) =
        single_list(&[ListLevel::new(ListNumberFormat::LowerRoman).start(u32::MAX)]);
    add_items(&mut doc, id, &[0]);
    assert_eq!(markers(&doc), [format!("{}.", u32::MAX)]);
}

// Known failures.

#[test]
fn items_can_be_added_to_a_libreoffice_report() {
    let mut doc = corpus_doc("libreoffice/report.docx");
    doc.try_add_bullet_list_item("New point", 0).unwrap();
    doc.try_add_numbered_list_item("New step", 0).unwrap();

    let doc = reopen(&mut doc);
    let mut expected = report_list_lines(SYMBOL_BULLET);
    expected.extend([
        format!("{SYMBOL_BULLET} New point"),
        "4. New step".to_owned(),
    ]);
    assert_eq!(list_lines(&doc), expected);
}

#[test]
fn every_list_edit_works_on_a_libreoffice_report() {
    // LibreOffice keeps the numbering of its List Bullet style and drops the
    // level's link back to the style: a one-way link, which Word reads.
    let mut doc = corpus_doc("libreoffice/report.docx");
    let point = doc
        .paragraph(paragraph_starting(&doc, "Subscription revenue"))
        .unwrap();
    assert_eq!(point.style_id(), Some("ListBullet"));
    let effective = doc.effective_paragraph_properties(&point);
    let (id, level) = (effective.num_id.unwrap(), effective.num_ilvl.unwrap_or(0));
    assert_eq!(
        doc.numbering_level(id, level).unwrap().paragraph_style,
        None
    );
    doc.validate_numbering_graph().unwrap();

    doc.add_bullet_list_item("New point", 0);
    let step = doc.paragraph_count();
    doc.add_numbered_list_item("New step", 0);
    doc.restart_numbering(step, 5).unwrap();
    assert_ne!(doc.add_list_definition(&[ListLevel::decimal()]), 0);
    doc.add_numbering_definition(&[ListLevel::bullet()])
        .unwrap();
    let mut unlinked = corpus_doc("libreoffice/report.docx");
    unlinked
        .unlink_style_from_numbering("ListBullet", id, level)
        .unwrap();
    let style = unlinked.style("ListBullet").unwrap();
    let properties = style.paragraph_properties().unwrap();
    assert_eq!(
        (properties.num_id, properties.num_ilvl),
        (None, None),
        "unlinking removes the style's side of a one-way link"
    );
    unlinked.validate_numbering_graph().unwrap();
    doc.link_style_to_numbering("ListBullet", id, level)
        .unwrap();
    assert_eq!(
        doc.numbering_level(id, level).unwrap().paragraph_style,
        Some("ListBullet"),
        "linking completes the link"
    );
    doc.validate_numbering_graph().unwrap();

    let doc = reopen(&mut doc);
    let lines = list_lines(&doc);
    assert_eq!(
        lines[lines.len() - 2..],
        [
            format!("{SYMBOL_BULLET} New point"),
            "5. New step".to_owned()
        ]
    );
    doc.validate_numbering_graph().unwrap();
}

// Fails today: Word writes w16cid:durableId on every list instance, and a restart refuses an instance with attributes it does not model. Issue: #29
#[test]
#[should_panic(expected = "cannot project unsupported override source")]
fn a_new_numbered_list_can_be_started_in_a_word_report() {
    let mut doc = corpus_doc("word/report.docx");
    doc.try_add_numbered_list_item_with_restart("First of a new list", 0, true)
        .unwrap();
    doc.try_add_numbered_list_item("Second of the new list", 0)
        .unwrap();

    let doc = reopen(&mut doc);
    let lines = list_lines(&doc);
    assert_eq!(
        lines[lines.len() - 2..],
        ["1. First of a new list", "2. Second of the new list"]
    );
}

#[test]
fn markdown_shows_word_list_paragraphs_as_list_items() {
    let markdown = corpus_doc("word/report.docx").to_markdown();
    let line = |prefix: &str| {
        markdown
            .lines()
            .find(|line| line.contains(prefix))
            .unwrap_or_else(|| panic!("no line holds {prefix:?}"))
            .to_owned()
    };
    assert_eq!(
        line("Subscription revenue"),
        "- Subscription revenue reached 4.2 million euros, up from 3.7 million."
    );
    assert_eq!(
        line("Hire two"),
        "1. Hire two support engineers for the evening shift."
    );
}

/// The list element (`<ul>` or `<ol>`) that holds the HTML item showing
/// `text`: the last list tag opened before it.
fn html_list_of<'a>(html: &'a str, text: &str) -> &'a str {
    let at = html
        .find(text)
        .unwrap_or_else(|| panic!("no HTML holds {text:?}"));
    let before = &html[..at];
    let ul = before.rfind("<ul>");
    let ol = before.rfind("<ol>");
    let closed = before.rfind("</ul>").max(before.rfind("</ol>"));
    match ul.max(ol) {
        Some(open) if Some(open) > closed => {
            if ul == Some(open) {
                "ul"
            } else {
                "ol"
            }
        }
        _ => "none",
    }
}

#[test]
fn exports_show_the_report_lists_of_every_producer() {
    // Word, LibreOffice and python-docx number these paragraphs through
    // their List Bullet and List Number styles, Google Docs on the paragraphs.
    for name in [
        "word/report.docx",
        "google/report.docx",
        "libreoffice/report.docx",
        "authored/report.docx",
    ] {
        let doc = corpus_doc(name);
        let markdown = doc.to_markdown();
        for item in [
            "- Subscription revenue reached 4.2 million euros, up from 3.7 million.",
            "- The Lyon warehouse opened on schedule and serves the southern region.",
            "1. Hire two support engineers for the evening shift.",
        ] {
            assert!(
                markdown.lines().any(|line| line == item),
                "{name}: Markdown shows {item:?}:\n{markdown}"
            );
        }
        let html = doc.to_html_fragment();
        assert_eq!(
            html_list_of(&html, "Subscription revenue reached"),
            "ul",
            "{name}"
        );
        assert_eq!(
            html_list_of(&html, "Hire two support engineers"),
            "ol",
            "{name}"
        );
    }
}

#[test]
fn exports_keep_headings_numbered_through_their_style_as_headings() {
    // Word numbers its headings by linking the heading styles to a list.
    let (doc, _) = numbered_headings();
    let markdown = doc.to_markdown();
    for line in ["# Introduction", "## Scope", "# Method"] {
        assert!(
            markdown.lines().any(|candidate| candidate == line),
            "Markdown keeps {line:?}:\n{markdown}"
        );
    }
    let html = doc.to_html_fragment();
    assert!(html.contains("<h1>Introduction</h1>"), "{html}");
    assert!(html.contains("<h2>Scope</h2>"), "{html}");
    assert!(!html.contains("<ol>"), "{html}");
}

#[test]
fn exports_follow_a_style_numbered_through_its_based_on_chain_or_the_default_style() {
    let (mut doc, id) = single_list(&[ListLevel::bullet()]);
    doc.add_style(StyleBuilder::paragraph("Points", "Points"))
        .unwrap();
    doc.link_style_to_numbering("Points", id, 0).unwrap();
    doc.add_style(StyleBuilder::paragraph("SubPoints", "Sub points").based_on("Points"))
        .unwrap();
    doc.add_paragraph("inherited point").style("SubPoints");
    assert_eq!(markers(&doc), ["•"]);
    assert!(
        doc.to_markdown()
            .lines()
            .any(|line| line == "- inherited point")
    );
    assert_eq!(
        html_list_of(&doc.to_html_fragment(), "inherited point"),
        "ul"
    );

    let (mut doc, id) = single_list(&[ListLevel::decimal()]);
    doc.link_style_to_numbering("Normal", id, 0).unwrap();
    doc.add_paragraph("default style step");
    assert_eq!(markers(&doc), ["1."]);
    assert!(
        doc.to_markdown()
            .lines()
            .any(|line| line == "1. default style step")
    );
}

#[test]
fn exports_leave_a_level_without_marker_out_of_lists() {
    let (mut doc, id) = single_list(&[ListLevel::new(ListNumberFormat::None)]);
    add_items(&mut doc, id, &[0]);
    assert_eq!(markers(&doc), [""]);
    assert!(doc.to_markdown().lines().any(|line| line == "item 1"));
    assert_eq!(html_list_of(&doc.to_html_fragment(), "item 1"), "none");
}

#[test]
fn exports_keep_a_list_open_around_its_unmarked_sub_paragraphs() {
    // Contracts number their clauses and leave the sub-paragraphs unmarked
    // on a `none` level: layout keeps counting the clauses around them.
    let (mut doc, id) =
        single_list(&[ListLevel::decimal(), ListLevel::new(ListNumberFormat::None)]);
    add_items(&mut doc, id, &[0, 1, 0, 1, 0]);
    assert_eq!(markers(&doc), ["1.", "", "2.", "", "3."]);

    let markdown = doc.to_markdown();
    let lines: Vec<&str> = markdown.lines().filter(|line| !line.is_empty()).collect();
    assert_eq!(
        lines,
        [
            "1. item 1",
            "   item 2",
            "1. item 3",
            "   item 4",
            "1. item 5"
        ],
        "one list, each sub-paragraph under its clause:\n{markdown}"
    );
    let html = doc.to_html_fragment();
    assert_eq!(html.matches("<ol>").count(), 1, "{html}");
    assert!(html.contains("<li>item 1<p>item 2</p></li>"), "{html}");
}

#[test]
fn exports_keep_counting_a_clause_whose_body_text_follows_its_sub_items() {
    // A clause, its sub-item (a), then body text of the clause on a `none`
    // level shallower than the sub-item, then the next clause.
    let (mut doc, id) = single_list(&[
        ListLevel::decimal(),
        ListLevel::new(ListNumberFormat::None),
        ListLevel::new(ListNumberFormat::LowerLetter).level_text("%3."),
    ]);
    add_items(&mut doc, id, &[0, 2, 1, 0]);
    assert_eq!(markers(&doc), ["1.", "a.", "", "2."]);

    let markdown = doc.to_markdown();
    assert!(
        !markdown.lines().any(|line| line == "item 3"),
        "the body text stays in the list:\n{markdown}"
    );
    let html = doc.to_html_fragment();
    // One outer list from the first clause to the second.
    let outer = html.find("<ol>").unwrap();
    let second = html.find("item 4").unwrap();
    assert!(
        !html[outer..second].contains("</ol>\n<p>item 3</p>\n<ol>"),
        "the outer list does not close around the body text: {html}"
    );
    assert_eq!(
        html.matches("</ol>").count(),
        html.matches("<ol>").count(),
        "{html}"
    );
    assert!(
        html[..second].rfind("<ol>").unwrap() < html.find("item 3").unwrap(),
        "no list opens again before the second clause: {html}"
    );
}

#[test]
fn exports_close_a_list_before_an_unmarked_paragraph_of_another_list() {
    // A body style linked to a `none` level of another list only borrows its
    // indent: layout draws it outside the bullet list before it.
    let (mut doc, bullets) = single_list(&[ListLevel::bullet()]);
    let indents = doc
        .add_numbering_definition(&[ListLevel::new(ListNumberFormat::None)])
        .unwrap();
    let indents = doc.add_numbering_instance(indents, &[]).unwrap();
    add_items(&mut doc, bullets, &[0]);
    doc.add_paragraph("indented body").numbering(indents, 0);
    assert_eq!(markers(&doc), ["•", ""]);

    let markdown = doc.to_markdown();
    assert!(
        markdown.lines().any(|line| line == "indented body"),
        "a plain paragraph after the list:\n{markdown}"
    );
    let html = doc.to_html_fragment();
    assert!(html.contains("</ul>\n<p>indented body</p>"), "{html}");
}

#[test]
fn odt_export_reports_a_definition_start_a_swapped_in_level_does_not_change() {
    // Layout counts from the definition's start, 3, whatever the swapped-in
    // level says; ODT resets it, and says so.
    let (mut doc, numbered) = single_list(&[ListLevel::decimal().start(3)]);
    let swapped = doc
        .add_numbering_instance(
            definition_of(&doc, numbered),
            &[NumberingLevelOverride::new(0).replacement(ListLevel::decimal().start(1))],
        )
        .unwrap();
    add_items(&mut doc, swapped, &[0]);
    assert_eq!(markers(&doc), ["3."]);
    let start_diagnostic = |doc: &Document| {
        doc.to_odt_bytes()
            .unwrap()
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("custom list start value"))
    };
    // A start override is counted first: 1 here, which ODT keeps.
    let (mut overridden, plain) = single_list(&[ListLevel::decimal().start(3)]);
    let one = overridden
        .add_numbering_instance(
            definition_of(&overridden, plain),
            &[NumberingLevelOverride::new(0).start(1)],
        )
        .unwrap();
    add_items(&mut overridden, one, &[0]);
    assert_eq!(markers(&overridden), ["1."]);
    assert!(!start_diagnostic(&overridden), "no false reset diagnostic");
    let odt = doc.to_odt_bytes().unwrap();
    assert!(
        odt.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("custom list start value")),
        "{:?}",
        odt.diagnostics
            .iter()
            .map(|diagnostic| &diagnostic.message)
            .collect::<Vec<_>>()
    );
}

#[test]
fn markdown_starts_a_swapped_in_level_where_layout_does() {
    let (mut doc, numbered) = single_list(&[ListLevel::decimal()]);
    let swapped = doc
        .add_numbering_instance(
            definition_of(&doc, numbered),
            &[NumberingLevelOverride::new(0).replacement(ListLevel::decimal().start(5))],
        )
        .unwrap();
    add_items(&mut doc, swapped, &[0]);
    let marker = markers(&doc).remove(0);
    assert!(
        doc.to_markdown()
            .lines()
            .any(|line| line == format!("{marker} item 1")),
        "Markdown starts at {marker}: {}",
        doc.to_markdown()
    );
}

#[test]
fn exports_show_the_level_an_instance_swaps_in() {
    let (mut doc, numbered) = single_list(&[ListLevel::decimal()]);
    let dashed = doc
        .add_numbering_instance(
            definition_of(&doc, numbered),
            &[NumberingLevelOverride::new(0).replacement(ListLevel::bullet().level_text("–"))],
        )
        .unwrap();
    add_items(&mut doc, dashed, &[0]);
    assert_eq!(markers(&doc), ["–"]);

    assert!(doc.to_markdown().lines().any(|line| line == "- item 1"));
    assert_eq!(html_list_of(&doc.to_html_fragment(), "item 1"), "ul");
    let odt = doc.to_odt_bytes().unwrap();
    let reread = Document::from_odt_bytes(&odt.bytes).unwrap().document;
    let (id, _) = reread
        .paragraph(0)
        .unwrap()
        .numbering()
        .expect("a list item");
    assert_eq!(
        reread.numbering_is_bullet(id),
        Some(true),
        "the ODT list is a bullet list"
    );
}

#[test]
fn lettered_lists_go_on_with_doubled_letters_after_z() {
    let (mut doc, id) = single_list(&[ListLevel::new(ListNumberFormat::LowerLetter)]);
    add_items(&mut doc, id, &[0; 28]);
    assert_eq!(markers(&doc)[24..26], ["y.", "z."]);
    assert_eq!(markers(&doc)[26..], ["aa.", "bb."]);
}

#[test]
fn lettered_lists_triple_their_letters_after_zz_in_both_cases() {
    for (format, expected) in [
        (ListNumberFormat::LowerLetter, ["zz.", "aaa.", "bbb."]),
        (ListNumberFormat::UpperLetter, ["ZZ.", "AAA.", "BBB."]),
    ] {
        let (mut doc, id) = single_list(&[ListLevel::new(format.clone()).start(52)]);
        add_items(&mut doc, id, &[0; 3]);
        assert_eq!(markers(&doc), expected, "{format:?}");
    }
}

#[test]
fn letter_markers_start_again_after_thirty_repeats() {
    // A guard against a hostile start value: 780 is 30 "z", then "a" again.
    let (mut doc, id) = single_list(&[ListLevel::new(ListNumberFormat::LowerLetter).start(780)]);
    add_items(&mut doc, id, &[0; 2]);
    assert_eq!(
        markers(&doc),
        [format!("{}.", "z".repeat(30)), "a.".to_owned()]
    );

    let (mut doc, id) =
        single_list(&[ListLevel::new(ListNumberFormat::LowerLetter).start(u32::MAX)]);
    add_items(&mut doc, id, &[0]);
    assert_eq!(markers(&doc), ["uuuuuuuuuu."]);
}

#[test]
fn a_list_whose_instance_swaps_in_a_bullet_level_reads_as_a_bullet_list() {
    let (mut doc, numbered) = single_list(&[ListLevel::decimal()]);
    let dashed = doc
        .add_numbering_instance(
            definition_of(&doc, numbered),
            &[NumberingLevelOverride::new(0).replacement(ListLevel::bullet().level_text("–"))],
        )
        .unwrap();
    add_items(&mut doc, dashed, &[0]);
    assert_eq!(doc.numbering_is_bullet(dashed), Some(true));
    assert_eq!(doc.numbering_is_bullet(numbered), Some(false));

    // A save renumbers the instances made in this session by first use, so
    // the reopened list is found through its paragraph.
    let doc = reopen(&mut doc);
    assert_eq!(markers(&doc), ["–"]);
    let (id, _) = doc.paragraph(0).unwrap().numbering().unwrap();
    assert_eq!(doc.numbering_is_bullet(id), Some(true));
}

#[test]
fn a_bullet_list_whose_instance_swaps_in_a_numbered_level_reads_as_numbered() {
    let (mut doc, bullets) = single_list(&[ListLevel::bullet()]);
    let numbered = doc
        .add_numbering_instance(
            definition_of(&doc, bullets),
            &[NumberingLevelOverride::new(0).replacement(ListLevel::decimal())],
        )
        .unwrap();
    let restarted = doc
        .add_numbering_instance(
            definition_of(&doc, bullets),
            &[NumberingLevelOverride::new(0).start(3)],
        )
        .unwrap();
    add_items(&mut doc, numbered, &[0]);
    assert_eq!(doc.numbering_is_bullet(numbered), Some(false));
    assert_eq!(
        doc.numbering_is_bullet(restarted),
        Some(true),
        "a start override keeps the definition's level"
    );
    assert_eq!(doc.numbering_is_bullet(bullets), Some(true));

    let doc = reopen(&mut doc);
    assert_eq!(markers(&doc), ["1."]);
    let (id, _) = doc.paragraph(0).unwrap().numbering().unwrap();
    assert_eq!(doc.numbering_is_bullet(id), Some(false));
}

#[test]
fn a_swapped_in_level_without_a_format_reads_as_numbered() {
    // A level with no w:numFmt is decimal, as layout draws it.
    let (mut doc, bullets) = single_list(&[ListLevel::bullet()]);
    let swapped = doc
        .add_numbering_instance(
            definition_of(&doc, bullets),
            &[NumberingLevelOverride::new(0).replacement(ListLevel::decimal())],
        )
        .unwrap();
    add_items(&mut doc, swapped, &[0]);
    let mut package =
        OpcPackage::from_reader(Cursor::new(doc.to_bytes().unwrap())).expect("package opens");
    let xml = String::from_utf8(package.get_part("/word/numbering.xml").unwrap().to_vec()).unwrap();
    let override_start = xml.find("<w:lvlOverride").expect("the swapped-in level");
    let format = r#"<w:numFmt w:val="decimal"/>"#;
    let at = override_start + xml[override_start..].find(format).expect("its format");
    let xml = format!("{}{}", &xml[..at], &xml[at + format.len()..]);
    package.set_part("/word/numbering.xml", xml.into_bytes());
    let mut bytes = Cursor::new(Vec::new());
    package.write_to(&mut bytes).unwrap();
    let doc = Document::from_bytes(&bytes.into_inner()).unwrap();

    assert_eq!(markers(&doc), ["1."]);
    let (id, _) = doc.paragraph(0).unwrap().numbering().unwrap();
    assert_eq!(doc.numbering_is_bullet(id), Some(false));
}
